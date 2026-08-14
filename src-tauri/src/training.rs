//! JSONL traces of model input/output plus mechanically detected mistakes.
//!
//! These logs are the training set for improving prompt/grammar quality:
//! - `generations.jsonl` — every dictate / fn+1 / fn+2 / fn+3 result, with draft vs
//!   final, repair flags, and mistake tags (missing sections, dropped names, unsafe polish)
//! - a later recapture of the same field that looks like an edit becomes a
//!   `user_correction` gold label against the previous generation
//!
//! Files live in `~/Library/Application Support/voice-flow/training/` (mode 0700).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::config::{self, AppConfig};

const MAX_LOG_BYTES: u64 = 12 * 1024 * 1024;
const CORRECTION_WINDOW_SECS: u64 = 2 * 60 * 60;
const TEMPLATE_HEADERS: [&str; 10] = [
    "title",
    "role",
    "stance",
    "task",
    "context",
    "inputs",
    "available",
    "output",
    "requirements",
    "constraints",
];

static LAST_PASTE: Mutex<Option<LastPaste>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingMistake {
    #[serde(rename = "type")]
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProcessTrace {
    #[serde(default)]
    pub draft: Option<String>,
    #[serde(default)]
    pub repaired: bool,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub polish_rejected: bool,
    #[serde(default)]
    pub mistakes: Vec<TrainingMistake>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingEvent {
    pub id: String,
    pub ts: u64,
    pub kind: String,
    pub mode: String,
    pub processing_mode: String,
    pub model: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    pub output: String,
    #[serde(default)]
    pub repaired: bool,
    #[serde(default)]
    pub used_fallback: bool,
    #[serde(default)]
    pub polish_rejected: bool,
    #[serde(default)]
    pub mistakes: Vec<TrainingMistake>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default)]
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingStats {
    pub generations: u64,
    pub with_mistakes: u64,
    pub user_corrections: u64,
    pub log_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LastPaste {
    id: String,
    mode: String,
    output: String,
    app_name: Option<String>,
    ts: u64,
    consumed: bool,
}

pub fn mistake(kind: &str, detail: impl Into<String>) -> TrainingMistake {
    TrainingMistake {
        kind: kind.to_string(),
        detail: detail.into(),
    }
}

pub fn training_dir() -> Result<PathBuf, String> {
    let dir = config::data_dir()?.join("training");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

fn generations_path() -> Result<PathBuf, String> {
    Ok(training_dir()?.join("generations.jsonl"))
}

fn last_paste_path() -> Result<PathBuf, String> {
    Ok(training_dir()?.join("last_paste.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_last_paste() -> Option<LastPaste> {
    if let Ok(guard) = LAST_PASTE.lock() {
        if guard.is_some() {
            return guard.clone();
        }
    }
    let path = last_paste_path().ok()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_last_paste(paste: &LastPaste) {
    if let Ok(mut guard) = LAST_PASTE.lock() {
        *guard = Some(paste.clone());
    }
    if let Ok(path) = last_paste_path() {
        if let Ok(raw) = serde_json::to_string_pretty(paste) {
            let _ = crate::config::atomic_write_private(&path, &raw);
        }
    }
}

fn append_event(event: &TrainingEvent) -> Result<(), String> {
    let path = generations_path()?;
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let rotated = path.with_extension("jsonl.1");
            let _ = fs::rename(&path, rotated);
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Could not open training log: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| format!("Could not write training log: {e}"))
}

pub fn log_generation(
    config: &AppConfig,
    mode: &str,
    app_name: Option<&str>,
    source: &str,
    output: &str,
    trace: &ProcessTrace,
    context: Option<&str>,
    latency_ms: u64,
) -> String {
    let id = Uuid::new_v4().to_string();
    let event = TrainingEvent {
        id: id.clone(),
        ts: now_secs(),
        kind: "generation".into(),
        mode: mode.to_string(),
        processing_mode: config.processing_mode.clone(),
        model: config.llm_model.clone(),
        provider: config.llm_provider.clone(),
        app_name: app_name.map(ToOwned::to_owned),
        source: source.to_string(),
        draft: trace.draft.clone(),
        output: output.to_string(),
        repaired: trace.repaired,
        used_fallback: trace.used_fallback,
        polish_rejected: trace.polish_rejected,
        mistakes: trace.mistakes.clone(),
        parent_id: None,
        gold: None,
        context: context
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| truncate_chars(text, 4000)),
        latency_ms,
    };
    let _ = append_event(&event);
    save_last_paste(&LastPaste {
        id: id.clone(),
        mode: mode.to_string(),
        output: output.to_string(),
        app_name: app_name.map(ToOwned::to_owned),
        ts: event.ts,
        consumed: false,
    });
    id
}

pub fn log_error(
    config: &AppConfig,
    mode: &str,
    app_name: Option<&str>,
    source: &str,
    err: &str,
) {
    let event = TrainingEvent {
        id: Uuid::new_v4().to_string(),
        ts: now_secs(),
        kind: "generation".into(),
        mode: mode.to_string(),
        processing_mode: config.processing_mode.clone(),
        model: config.llm_model.clone(),
        provider: config.llm_provider.clone(),
        app_name: app_name.map(ToOwned::to_owned),
        source: source.to_string(),
        draft: None,
        output: String::new(),
        repaired: false,
        used_fallback: false,
        polish_rejected: false,
        mistakes: vec![mistake("error", err)],
        parent_id: None,
        gold: None,
        context: None,
        latency_ms: 0,
    };
    let _ = append_event(&event);
}

/// If the newly captured field looks like an edit of the last paste, record it as gold.
pub fn note_captured_source(next_mode: &str, app_name: Option<&str>, captured: &str) {
    let captured = captured.trim();
    if captured.is_empty() {
        return;
    }
    let Some(mut last) = load_last_paste() else {
        return;
    };
    if last.consumed || last.output.trim() == captured {
        return;
    }
    if now_secs().saturating_sub(last.ts) > CORRECTION_WINDOW_SECS {
        return;
    }
    if !apps_match(last.app_name.as_deref(), app_name) {
        return;
    }
    if !is_correction_pair(&last.mode, next_mode, &last.output, captured) {
        return;
    }

    let event = TrainingEvent {
        id: Uuid::new_v4().to_string(),
        ts: now_secs(),
        kind: "user_correction".into(),
        mode: last.mode.clone(),
        processing_mode: String::new(),
        model: String::new(),
        provider: String::new(),
        app_name: app_name.map(ToOwned::to_owned),
        source: String::new(),
        draft: None,
        output: last.output.clone(),
        repaired: false,
        used_fallback: false,
        polish_rejected: false,
        mistakes: vec![mistake(
            "user_edit",
            "User changed the pasted text before the next shortcut — treat `gold` as the desired output.",
        )],
        parent_id: Some(last.id.clone()),
        gold: Some(captured.to_string()),
        context: None,
        latency_ms: 0,
    };
    let _ = append_event(&event);
    crate::store::learn_dictionary_from_correction(&last.output, captured);
    last.consumed = true;
    save_last_paste(&last);
}

fn apps_match(previous: Option<&str>, current: Option<&str>) -> bool {
    match (previous, current) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => true,
    }
}

pub fn is_correction_pair(last_mode: &str, next_mode: &str, model_output: &str, captured: &str) -> bool {
    match (last_mode, next_mode) {
        ("vibe_text", "vibe_text") => {
            looks_like_prompt(model_output)
                && looks_like_prompt(captured)
                && content_jaccard(model_output, captured) >= 0.25
        }
        ("correct_text", "correct_text")
        | ("dictate", "correct_text")
        | ("vibe_text", "correct_text") => content_jaccard(model_output, captured) >= 0.4,
        ("meeting_answer", "correct_text") | ("meeting_answer", "vibe_text") => {
            content_jaccard(model_output, captured) >= 0.4
        }
        _ => false,
    }
}

fn looks_like_prompt(text: &str) -> bool {
    text.contains("**Title**") && text.contains("**Task**")
}

fn content_jaccard(a: &str, b: &str) -> f32 {
    let left = content_tokens(a);
    let right = content_tokens(b);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(&right).count() as f32;
    let union = left.union(&right).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn content_tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter_map(|raw| {
            let token = raw.trim().to_lowercase();
            if token.len() < 3 || TEMPLATE_HEADERS.contains(&token.as_str()) {
                None
            } else {
                Some(token)
            }
        })
        .collect()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push('…');
    truncated
}

#[tauri::command]
pub fn get_training_stats() -> Result<TrainingStats, String> {
    let path = generations_path()?;
    let mut generations = 0;
    let mut with_mistakes = 0;
    let mut user_corrections = 0;
    if path.exists() {
        let file = fs::File::open(&path).map_err(|e| e.to_string())?;
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<TrainingEvent>(&line) else {
                continue;
            };
            if event.kind == "user_correction" {
                user_corrections += 1;
            } else {
                generations += 1;
                if !event.mistakes.is_empty() {
                    with_mistakes += 1;
                }
            }
        }
    }
    Ok(TrainingStats {
        generations,
        with_mistakes,
        user_corrections,
        log_path: path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn open_training_folder() -> Result<(), String> {
    let dir = training_dir()?;
    std::process::Command::new("open")
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("Could not open training folder: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_edits_count_as_gold_labels() {
        let original = "**Title**\nFix Flow dictation\n\n**Task**\n- Keep fn paste raw\n- Preserve Wispr Flow names";
        let edited = "**Title**\nFix Flow dictation\n\n**Task**\n- Keep fn paste raw transcript\n- Preserve Wispr Flow names exactly";
        assert!(is_correction_pair("vibe_text", "vibe_text", original, edited));
    }

    #[test]
    fn unrelated_new_request_is_not_a_correction() {
        let original = "**Title**\nFix Flow dictation\n\n**Task**\n- Keep fn paste raw";
        let next = "rewrite the onboarding email for Nimbus";
        assert!(!is_correction_pair("vibe_text", "vibe_text", original, next));
    }

    #[test]
    fn edited_dictation_then_fn2_is_stt_gold() {
        let spoken = "tomorrow check flow api latency";
        let edited = "Tomorrow check Flow API latency.";
        assert!(is_correction_pair(
            "dictate",
            "correct_text",
            spoken,
            edited
        ));
    }
}
