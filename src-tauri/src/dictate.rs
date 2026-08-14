use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::cloud_api::{self, ProcessPayload};
use crate::config::{llm_key_configured, load_config, AppConfig};
use crate::dictation_post::{
    expand_snippets, is_safe_light_cleanup, is_unusable_stt, looks_like_edit_command,
    sanitize_stt_transcript,
};
use crate::focus::{
    capture_field_text_for_transform, hide_bubble, paste_into_target_app, take_selected_text,
    target_app_name,
};
use crate::store::{add_history, load_store};
use crate::stt_local;
use crate::training::{self, ProcessTrace};
use crate::vibe_context;

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

const REQUIRED_VIBE_SECTIONS: [&str; 10] = [
    "**Title**",
    "**Role & Stance**",
    "**Task**",
    "**Context**",
    "**Inputs Available**",
    "**Output Requirements**",
    "**Constraints / Do-nots**",
    "**Examples / References**",
    "**Execution Checklist**",
    "**Conflict Resolution**",
];

const WRAPPER_PREAMBLE_PREFIXES: [&str; 9] = [
    "here is",
    "here's",
    "here are",
    "sure,",
    "sure!",
    "certainly,",
    "certainly!",
    "below is",
    "of course,",
];

#[derive(Serialize)]
pub struct UiPrefs {
    pub processing_mode: String,
    pub interaction_sounds: bool,
}

#[tauri::command]
pub async fn get_config() -> Result<AppConfig, String> {
    Ok(load_config())
}

#[tauri::command]
pub async fn get_ui_prefs() -> Result<UiPrefs, String> {
    let cfg = load_config();
    Ok(UiPrefs {
        processing_mode: cfg.processing_mode,
        interaction_sounds: cfg.interaction_sounds,
    })
}

#[tauri::command]
pub async fn save_user_config(config: AppConfig) -> Result<(), String> {
    crate::config::save_config(&config)
}

/// Returns remaining setup steps. Empty means processing is configured.
#[tauri::command]
pub async fn get_readiness() -> Result<Vec<String>, String> {
    let cfg = load_config();
    let mut gaps = Vec::new();

    if cloud_api::uses_cloud_llm(&cfg) {
        if cfg.flow_api_url.trim().is_empty() {
            gaps.push(
                "FLOW_API_URL missing. Deploy with cloud/deploy.sh or switch Processing mode to local."
                    .into(),
            );
        }
        if cfg.flow_api_key.trim().is_empty() {
            gaps.push(
                "FLOW_API_KEY missing. Re-run cloud/deploy.sh or switch Processing mode to local."
                    .into(),
            );
        }
        if cloud_api::uses_cloud(&cfg) {
            return Ok(gaps);
        }
    }

    if cloud_api::uses_local_stt(&cfg) {
        gaps.extend(stt_local::readiness_gaps(&cfg));
    }
    if !cloud_api::uses_cloud_llm(&cfg) && !llm_key_configured(&cfg) {
        gaps.push(
            "Add DeepSeek API key in Settings, or DEEPSEEK_API_KEY in Application Support/voice-flow/.env."
                .into(),
        );
    }
    Ok(gaps)
}

#[tauri::command]
pub async fn transcribe_partial(audio_base64: String) -> Result<String, String> {
    let config = load_config();
    ensure_providers(&config, "dictate")?;

    let audio_bytes = STANDARD
        .decode(audio_base64.trim())
        .map_err(|e| format!("Invalid audio data: {e}"))?;
    if audio_bytes.len() < 2000 {
        return Err(format!(
            "Audio too short for live partial ({} bytes).",
            audio_bytes.len()
        ));
    }

    let text = if cloud_api::uses_cloud(&config) {
        cloud_api::transcribe(&config, audio_base64.trim()).await?
    } else {
        stt_local::transcribe(&config, audio_bytes).await?
    };
    let text = sanitize_stt_transcript(&text);
    if text.is_empty() || is_unusable_stt(&text) {
        return Err("Speech-to-Text returned empty text (no speech detected).".into());
    }
    Ok(text)
}

#[tauri::command]
pub async fn process_dictation(
    app: AppHandle,
    audio_base64: String,
    mode: String,
) -> Result<String, String> {
    let result = process_dictation_inner(&app, audio_base64, mode, None).await;
    if result
        .as_ref()
        .err()
        .is_some_and(|err| !is_no_speech_error(err))
    {
        hide_bubble(&app);
    }
    result
}

#[tauri::command]
pub async fn process_transcript(
    app: AppHandle,
    transcript: String,
    mode: String,
) -> Result<String, String> {
    let result = process_dictation_inner(&app, String::new(), mode, Some(transcript)).await;
    if result
        .as_ref()
        .err()
        .is_some_and(|err| !is_no_speech_error(err))
    {
        hide_bubble(&app);
    }
    result
}

#[tauri::command]
pub fn hide_bubble_window(app: AppHandle) -> Result<(), String> {
    hide_bubble(&app);
    Ok(())
}

fn is_no_speech_error(message: &str) -> bool {
    let text = message.to_lowercase();
    text.contains("no audio captured")
        || text.contains("no speech detected")
        || text.contains("empty text")
        || text.contains("audio too short")
        || text.contains("gcp speech returned empty text")
        || text.contains("cloud stt returned empty text")
        || text.contains("cloud process returned empty text")
}

async fn process_dictation_inner(
    app: &AppHandle,
    audio_base64: String,
    mode: String,
    pre_transcribed_text: Option<String>,
) -> Result<String, String> {
    let config = load_config();
    ensure_providers(&config, &mode)?;

    // Cloud Run LLM for text transforms. Full cloud also sends dictate audio for STT.
    // Hybrid dictation stays on the Mac for STT, then uses Cloud Run for cleanup.
    if cloud_api::uses_cloud_llm(&config)
        && matches!(mode.as_str(), "vibe_text" | "correct_text")
    {
        return process_via_cloud(app, &config, audio_base64, &mode).await;
    }
    if cloud_api::uses_cloud(&config)
        && mode == "dictate"
        && pre_transcribed_text.is_none()
    {
        return process_via_cloud(app, &config, audio_base64, &mode).await;
    }

    let audio_bytes = if audio_base64.trim().is_empty() {
        Vec::new()
    } else {
        STANDARD
            .decode(audio_base64.trim())
            .map_err(|e| format!("Invalid audio data: {e}"))?
    };

    let store = load_store();

    let raw_text = if let Some(text) = pre_transcribed_text {
        let original = text.trim().to_string();
        let text = sanitize_stt_transcript(&original);
        if text.is_empty() {
            if !original.is_empty() {
                training::log_error(
                    &config,
                    &mode,
                    target_app_name(app).as_deref(),
                    &original,
                    "STT hallucination discarded (prompt leak or [BLANK_AUDIO]).",
                );
            }
            return Err("No speech detected.".into());
        }
        text
    } else if !audio_bytes.is_empty() && audio_bytes.len() < 1500 {
        return Err(format!(
            "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
            audio_bytes.len()
        ));
    } else if audio_bytes.len() >= 1500 {
        let _ = app.emit("dictation-status", "transcribing");
        let text = if cloud_api::uses_cloud(&config) {
            cloud_api::transcribe(&config, audio_base64.trim()).await?
        } else {
            stt_local::transcribe(&config, audio_bytes).await?
        }
        .trim()
        .to_string();
        let text = sanitize_stt_transcript(&text);
        if text.is_empty() {
            return Err("Speech-to-Text returned empty text (no speech detected).".into());
        }
        text
    } else {
        String::new()
    };

    if !raw_text.is_empty() {
        let _ = app.emit("partial-transcript", &raw_text);
    }

    let app_name = target_app_name(app);
    let started = Instant::now();
    let mut source_text = raw_text.clone();
    let mut trace = ProcessTrace::default();
    // Modes: dictate (fn raw), vibe_text (fn+1), correct_text (fn+2), meeting_answer (fn+3).
    let final_text = match mode.as_str() {
        "correct_text" => {
            let selected = take_selected_text(app).ok_or_else(|| {
                "No text captured. Focus the field containing text to correct, then press fn+2."
                    .to_string()
            })?;
            training::note_captured_source("correct_text", app_name.as_deref(), &selected);
            let _ = app.emit("partial-transcript", &selected);
            let _ = app.emit("dictation-status", "correcting");
            source_text = selected.clone();
            polish_text(&config, &selected, app_name.as_deref(), &store).await?
        }
        // fn+1: select current text + project context → new Vibe Coding prompt
        "vibe_text" => {
            let selected = take_selected_text(app).ok_or_else(|| {
                "No text captured. Focus the field containing your rough request, then press fn+1."
                    .to_string()
            })?;
            training::note_captured_source("vibe_text", app_name.as_deref(), &selected);
            let _ = app.emit("partial-transcript", &selected);
            let _ = app.emit("dictation-status", "correcting");
            source_text = selected.clone();
            let (text, vibe_trace) = generate_vibe_prompt_from_existing_text(
                &config,
                &selected,
                app_name.as_deref(),
                &store,
            )
            .await?;
            trace = vibe_trace;
            text
        }
        _ => {
            if raw_text.is_empty() {
                return Err("No speech detected.".into());
            }
            finish_dictation_text(app, &config, &raw_text, &store).await?
        }
    };

    let final_text = final_text.trim().to_string();
    if final_text.is_empty() {
        training::log_error(
            &config,
            &mode,
            app_name.as_deref(),
            &source_text,
            "Model returned empty text.",
        );
        return Err("Model returned empty text.".into());
    }

    // Log before paste. fn+1 often failed only at paste (Cursor AX), and those runs
    // previously left no training row — so prompting looked "dead" with no evidence.
    training::log_generation(
        &config,
        &mode,
        app_name.as_deref(),
        &source_text,
        &final_text,
        &trace,
        None,
        started.elapsed().as_millis() as u64,
    );

    let _ = app.emit("dictation-status", "pasting");
    if let Err(err) = paste_into_target_app(app, &final_text) {
        training::log_error(
            &config,
            &mode,
            app_name.as_deref(),
            &source_text,
            &format!("Paste failed after generation: {err}"),
        );
        return Err(err);
    }
    if let Err(e) = add_history(&final_text, app_name, &mode) {
        let _ = app.emit(
            "dictation-warning",
            format!("Pasted successfully, but history save failed: {e}"),
        );
    }
    hide_bubble(app);

    let _ = app.emit("dictation-done", &final_text);
    Ok(final_text)
}

fn ensure_providers(config: &AppConfig, mode: &str) -> Result<(), String> {
    if cloud_api::uses_cloud_llm(config)
        && (config.flow_api_url.trim().is_empty() || config.flow_api_key.trim().is_empty())
    {
        return Err(
            "Cloud processing requires FLOW_API_URL and FLOW_API_KEY. Run cloud/deploy.sh, or switch Processing mode to local."
                .into(),
        );
    }
    if cloud_api::uses_cloud(config) {
        return Ok(());
    }
    if cloud_api::uses_local_stt(config)
        && mode_requires_stt(mode)
        && matches!(config.stt_provider.trim(), "groq" | "groq_whisper")
        && config.groq_api_key.trim().is_empty()
    {
        return Err(
            "Groq API key missing. Add it in Settings, or set GROQ_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    // Cleanup no longer degrades to the raw transcript, so a dictation with
    // `correct_english` on needs the LLM just as much as the text transforms do.
    let needs_llm = mode_requires_llm(mode) || (mode == "dictate" && config.correct_english);
    if !cloud_api::uses_cloud_llm(config) && needs_llm && !llm_key_configured(config) {
        return Err(
            "LLM API key missing. Add it in Settings, or set DEEPSEEK_API_KEY / XAI_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    if !cloud_api::uses_cloud_llm(config)
        && !matches!(
            config.llm_provider.trim(),
            "" | "deepseek" | "xai" | "openai_compatible"
        )
    {
        return Err(format!(
            "Unsupported LLM provider '{}'. Choose deepseek, xai, or openai_compatible.",
            config.llm_provider
        ));
    }
    Ok(())
}

fn mode_requires_llm(mode: &str) -> bool {
    matches!(mode, "vibe_text" | "correct_text" | "meeting_answer" | "edit_text")
}

fn mode_requires_stt(mode: &str) -> bool {
    matches!(mode, "dictate")
}

async fn process_via_cloud(
    app: &AppHandle,
    config: &AppConfig,
    audio_base64: String,
    mode: &str,
) -> Result<String, String> {
    let constitution = vibe_context::load_constitution();
    let project_context = vibe_context::load_project_context().unwrap_or_default();
    let skill_id = match mode {
        "vibe_text" => "vibe-prompt",
        "correct_text" => "grammar-correct",
        _ => "vibe-prompt",
    };
    let skill = vibe_context::load_skill_blurb(skill_id);
    let store = load_store();
    let dictionary: Vec<String> = store.dictionary.iter().map(|d| d.word.clone()).collect();

    let captured_selection = if mode == "dictate" {
        take_selected_text(app).unwrap_or_default()
    } else {
        String::new()
    };

    let (selected_text, audio) = if mode == "vibe_text" || mode == "correct_text" {
        let selected = take_selected_text(app).ok_or_else(|| {
            if mode == "vibe_text" {
                return "No text captured. Focus the field containing your rough request, then press fn+1."
                    .to_string();
            }
            if mode == "correct_text" {
                return "No text captured. Focus the field containing text to correct, then press fn+2."
                    .to_string();
            }
            "No text captured.".to_string()
        })?;
        training::note_captured_source(mode, target_app_name(app).as_deref(), &selected);
        let _ = app.emit("partial-transcript", &selected);
        let _ = app.emit("dictation-status", "correcting");
        (Some(selected), None)
    } else {
        if audio_base64.trim().is_empty() {
            return Err("No speech detected. Hold fn and speak for raw dictation.".into());
        }
        let bytes = STANDARD
            .decode(audio_base64.trim())
            .map_err(|e| format!("Invalid audio data: {e}"))?;
        if bytes.len() < 1500 {
            return Err(format!(
                "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
                bytes.len()
            ));
        }
        let _ = app.emit("dictation-status", "transcribing");
        (None, Some(audio_base64.trim().to_string()))
    };

    let started = Instant::now();
    let processed = if mode == "dictate" && !config.correct_english {
        let raw = cloud_api::transcribe(config, audio.as_deref().unwrap_or_default()).await?;
        cloud_api::ProcessResult {
            text: raw.clone(),
            raw_transcript: Some(raw),
            trace: ProcessTrace::default(),
        }
    } else {
        let _ = app.emit("dictation-status", "correcting");
        let payload = ProcessPayload {
            mode,
            audio_base64: audio.as_deref(),
            selected_text: selected_text.as_deref(),
            project_context: Some(project_context.as_str()),
            constitution: Some(constitution.as_str()),
            skill: Some(skill.as_str()),
            language: Some(config.language.as_str()),
            dictionary,
        };
        cloud_api::process(config, payload).await?
    };
    if let Some(raw) = &processed.raw_transcript {
        if !raw.trim().is_empty() {
            let _ = app.emit("partial-transcript", raw.trim());
        }
    }

    let mut final_text = processed.text.trim().to_string();
    if mode == "dictate" {
        let raw = processed
            .raw_transcript
            .as_deref()
            .unwrap_or(final_text.as_str());
        final_text = post_process_dictation(
            app,
            config,
            raw,
            &final_text,
            &captured_selection,
            &store,
        )
        .await?;
    }
    if final_text.is_empty() {
        training::log_error(
            config,
            mode,
            target_app_name(app).as_deref(),
            selected_text.as_deref().unwrap_or(""),
            "Cloud Run returned empty text.",
        );
        return Err("Cloud Run returned empty text.".into());
    }

    let app_name = target_app_name(app);
    training::log_generation(
        config,
        mode,
        app_name.as_deref(),
        selected_text.as_deref().unwrap_or(""),
        &final_text,
        &processed.trace,
        Some(project_context.as_str()),
        started.elapsed().as_millis() as u64,
    );
    let _ = app.emit("dictation-status", "pasting");
    if let Err(err) = paste_into_target_app(app, &final_text) {
        training::log_error(
            config,
            mode,
            app_name.as_deref(),
            selected_text.as_deref().unwrap_or(""),
            &format!("Paste failed after generation: {err}"),
        );
        return Err(err);
    }
    if let Err(e) = add_history(&final_text, app_name, mode) {
        let _ = app.emit(
            "dictation-warning",
            format!("Pasted successfully, but history save failed: {e}"),
        );
    }
    hide_bubble(app);
    let _ = app.emit("dictation-done", &final_text);
    Ok(final_text)
}

#[tauri::command]
pub async fn verify_stt_connection() -> Result<String, String> {
    let config = load_config();
    if cloud_api::uses_cloud(&config) {
        return cloud_api::health(&config).await;
    }
    stt_local::verify(&config).await
}

#[tauri::command]
pub async fn verify_llm_connection() -> Result<String, String> {
    let config = load_config();
    if cloud_api::uses_cloud_llm(&config) {
        return cloud_api::health(&config).await;
    }
    if !llm_key_configured(&config) {
        return Err("No LLM API key configured.".into());
    }

    let base = config.llm_base_url.trim().trim_end_matches('/');
    let mut body = json!({
        "model": config.llm_model,
        "temperature": 0.0,
        "messages": [
            { "role": "user", "content": "Reply with exactly: ok" }
        ]
    });
    if config.llm_provider.trim().eq_ignore_ascii_case("deepseek") {
        body["thinking"] = json!({ "type": "disabled" });
    }

    let response = llm_client()
        .post(llm_url(base, "chat/completions"))
        .bearer_auth(config.llm_api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("LLM auth/API error ({status}): {body}"));
    }

    Ok(format!(
        "LLM OK — provider {}, model {} at {}.",
        config.llm_provider, config.llm_model, base
    ))
}

/// Pull DeepSeek key from MyGCP Secret Manager into local config + .env (never committed).
#[tauri::command]
pub async fn import_deepseek_from_gcloud() -> Result<String, String> {
    let mut config = load_config();
    let project = if config.gcp_project_id.trim().is_empty() {
        "project-ced3b331-e814-4d72-8bc".to_string()
    } else {
        config.gcp_project_id.clone()
    };

    let output = Command::new("gcloud")
        .args([
            "secrets",
            "versions",
            "access",
            "latest",
            "--secret=n8n-deepseek-api-key",
            &format!("--project={project}"),
        ])
        .output()
        .map_err(|e| format!("Could not run gcloud: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Failed to read Secret Manager n8n-deepseek-api-key: {stderr}"
        ));
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() || !key.starts_with("sk-") {
        return Err("Secret Manager returned an empty or unexpected DeepSeek key.".into());
    }

    config.llm_api_key = key.clone();
    config.llm_provider = "deepseek".into();
    crate::config::save_config(&config)?;

    let dir = crate::config::data_dir()?;
    let env_path = dir.join(".env");
    let mut env_body = String::new();
    if env_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&env_path) {
            for line in existing.lines() {
                if line.trim().starts_with("DEEPSEEK_API_KEY=") {
                    continue;
                }
                env_body.push_str(line);
                env_body.push('\n');
            }
        }
    }
    env_body.push_str(&format!("DEEPSEEK_API_KEY={key}\n"));
    crate::config::atomic_write_private(&env_path, &env_body)?;

    Ok("Imported DeepSeek key from Secret Manager into local config and voice-flow/.env.".into())
}

/// fn+1 entry point: select current text → load context → create a Vibe Coding prompt.
pub async fn process_vibe_text(app: AppHandle) -> Result<String, String> {
    let _ = app.emit("dictation-status", "correcting");
    let result = process_dictation_inner(&app, String::new(), "vibe_text".into(), None).await;
    if result.is_err() {
        hide_bubble(&app);
    }
    result
}

/// fn+2 entry point: select current text → grammar-correct it → paste back.
pub async fn process_correct_text(app: AppHandle) -> Result<String, String> {
    let _ = app.emit("dictation-status", "correcting");
    let result = process_dictation_inner(&app, String::new(), "correct_text".into(), None).await;
    if result.is_err() {
        hide_bubble(&app);
    }
    result
}

/// fn+3 entry point: answer the latest question detected in the live conversation transcript.
pub async fn process_meeting_answer(app: AppHandle) -> Result<String, String> {
    let config = load_config();
    ensure_providers(&config, "meeting_answer")?;
    let context = crate::meeting::latest_question_context(&app)?;
    let store = load_store();

    let _ = app.emit("partial-transcript", &context.question);
    let _ = app.emit("dictation-status", "answering");
    training::note_captured_source(
        "meeting_answer",
        target_app_name(&app).as_deref(),
        &context.question,
    );

    let started = Instant::now();
    let processed_trace;
    let final_text = if cloud_api::uses_cloud_llm(&config) {
        let dictionary = store.dictionary.iter().map(|d| d.word.clone()).collect();
        let payload = ProcessPayload {
            mode: "meeting_answer",
            audio_base64: None,
            selected_text: Some(context.question.as_str()),
            project_context: Some(context.transcript.as_str()),
            constitution: None,
            skill: None,
            language: Some(config.language.as_str()),
            dictionary,
        };
        let processed = cloud_api::process(&config, payload).await?;
        processed_trace = processed.trace;
        processed.text
    } else {
        processed_trace = ProcessTrace::default();
        answer_meeting_question(&config, &context.question, &context.transcript).await?
    };

    let final_text = final_text.trim().to_string();
    if final_text.is_empty() {
        training::log_error(
            &config,
            "meeting_answer",
            target_app_name(&app).as_deref(),
            &context.question,
            "Model returned empty answer.",
        );
        return Err("Model returned empty answer.".into());
    }

    let _ = app.emit("dictation-status", "pasting");
    paste_into_target_app(&app, &final_text)?;
    training::log_generation(
        &config,
        "meeting_answer",
        target_app_name(&app).as_deref(),
        &context.question,
        &final_text,
        &processed_trace,
        Some(context.transcript.as_str()),
        started.elapsed().as_millis() as u64,
    );
    if let Err(e) = add_history(&final_text, target_app_name(&app), "meeting_answer") {
        let _ = app.emit(
            "dictation-warning",
            format!("Pasted successfully, but history save failed: {e}"),
        );
    }
    hide_bubble(&app);
    let _ = app.emit("dictation-done", &final_text);
    Ok(final_text)
}

/// Capture frontmost app + current field text for fn+1 prompt creation.
pub fn prepare_vibe_text(app: &AppHandle) -> Result<String, String> {
    crate::focus::capture_frontmost_app(app)?;
    let text = capture_field_text_for_transform(app)?;
    training::note_captured_source("vibe_text", target_app_name(app).as_deref(), &text);
    Ok(text)
}

/// Capture frontmost app + current field text for fn+2 correction.
pub fn prepare_correct_text(app: &AppHandle) -> Result<String, String> {
    crate::focus::capture_frontmost_app(app)?;
    let text = capture_field_text_for_transform(app)?;
    training::note_captured_source("correct_text", target_app_name(app).as_deref(), &text);
    Ok(text)
}

/// fn+1 text path: turn rough text already in the focused field into a context-aware
/// canonical Vibe Coding prompt.
async fn generate_vibe_prompt_from_existing_text(
    config: &AppConfig,
    rough_text: &str,
    app_name: Option<&str>,
    store: &crate::store::FlowStore,
) -> Result<(String, ProcessTrace), String> {
    let constitution = vibe_context::load_constitution();
    let project_context = vibe_context::load_project_context()?;
    let skill = vibe_context::load_skill_blurb("vibe-prompt");
    let app_tone = if config.app_aware_tone {
        app_aware_instruction(app_name)
    } else {
        String::new()
    };
    let dict_words = store
        .dictionary
        .iter()
        .map(|d| d.word.clone())
        .collect::<Vec<_>>();
    let dict = dict_words.join(", ");

    let system = format!(
        r#"You turn the user's existing rough text into a perfect Vibe Coding prompt.

The rough text may be notes, a messy request, or an under-specified prompt. Optimize it into
the final prompt an AI coding agent will receive.

You are NOT implementing the task — you only write the prompt text.

Constitution:
{constitution}

Skill notes:
{skill}

Rules:
- Preserve the user's core goal and constraints
- Use relevant project context naturally; do not dump raw context
- Follow the canonical template from the constitution exactly: all ten sections, in order, none empty
- Use [FILL: ...] placeholders where specifics are unknown
- Do not invent features beyond the user's rough text and provided project context
- Return ONLY the final prompt — no transcript, preamble, quotes, or explanation
- Language preference code: {lang}
- App tone guidance: {app_tone}
- Preserve these dictionary terms exactly when present: {dict}"#,
        constitution = constitution,
        skill = skill,
        lang = config.language,
        app_tone = if app_tone.is_empty() {
            "none"
        } else {
            &app_tone
        },
        dict = if dict.is_empty() { "none" } else { &dict },
    );

    let user = format!(
        "Rough text from the active field:\n\n{rough_text}\n\n---\nProject context:\n\n{project_context}\n\n---\nCreate the optimized Vibe Coding prompt."
    );
    let draft = chat_completion(config, &system, &user, 2600).await?;
    repair_vibe_prompt_if_needed(
        config,
        &draft,
        &constitution,
        &project_context,
        rough_text,
        &dict_words,
        &config.language,
    )
    .await
}

async fn repair_vibe_prompt_if_needed(
    config: &AppConfig,
    draft: &str,
    constitution: &str,
    extra_context: &str,
    source: &str,
    dictionary: &[String],
    language: &str,
) -> Result<(String, ProcessTrace), String> {
    let section_mistakes = vibe_section_mistakes(draft);
    let dropped = dropped_proper_terms(source, draft, dictionary);
    let mut trace = ProcessTrace {
        draft: Some(draft.to_string()),
        ..Default::default()
    };
    for mistake in &section_mistakes {
        trace
            .mistakes
            .push(training::mistake(mistake.kind, mistake.section));
    }
    for term in &dropped {
        trace.mistakes.push(training::mistake("dropped_term", term));
    }
    if section_mistakes.is_empty() && dropped.is_empty() {
        return Ok((draft.trim().to_string(), trace));
    }

    let mut issues = section_repair_issues(&section_mistakes);
    if !dropped.is_empty() {
        issues.push(format!(
            "Missing exact source terms: {}. Restore these names exactly and use them consistently.",
            dropped.join(", ")
        ));
    }

    let system = format!(
        r#"You are a strict editor for Vibe Coding prompts.

Fix only the mechanically detected problems below. Otherwise preserve the draft wording and
structure. Do not shorten unrelated sections and do not add features.

Constitution:
{constitution}

Detected problems:
- {issues}

Rules:
- Keep every canonical template section present, in order, and non-empty
- Preserve exact proper nouns and product names
- Use [FILL: ...] placeholders where specifics are unknown
- Return ONLY the corrected final prompt text
- Language preference code: {language}"#,
        constitution = constitution,
        issues = issues.join("\n- "),
        language = language,
    );

    let user = format!(
        "DRAFT:\n\n{draft}\n\n---\nAdditional project context to weave in only if relevant:\n\n{}",
        if extra_context.trim().is_empty() {
            "none"
        } else {
            extra_context
        }
    );
    let repaired = chat_completion(config, &system, &user, 2600).await?;
    trace.repaired = true;
    for mistake in vibe_section_mistakes(&repaired) {
        trace.mistakes.push(training::mistake(
            match mistake.kind {
                "missing_section" => "remaining_missing_section",
                "empty_section" => "remaining_empty_section",
                "out_of_order_section" => "remaining_out_of_order_section",
                other => other,
            },
            mistake.section,
        ));
    }
    for term in dropped_proper_terms(source, &repaired, dictionary) {
        trace
            .mistakes
            .push(training::mistake("remaining_dropped_term", term));
    }
    Ok((repaired, trace))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VibeSectionMistake {
    kind: &'static str,
    section: &'static str,
}

fn section_body_is_empty(body: &str) -> bool {
    !body.chars().any(|c| c.is_ascii_alphanumeric())
}

fn vibe_section_mistakes(text: &str) -> Vec<VibeSectionMistake> {
    let positions: Vec<Option<usize>> = REQUIRED_VIBE_SECTIONS
        .iter()
        .map(|section| text.find(section))
        .collect();
    let mut present: Vec<(usize, usize)> = positions
        .iter()
        .enumerate()
        .filter_map(|(index, pos)| pos.map(|start| (index, start)))
        .collect();
    present.sort_by_key(|(_, start)| *start);

    let mut next_at = vec![text.len(); REQUIRED_VIBE_SECTIONS.len()];
    for (idx, &(section_index, _)) in present.iter().enumerate() {
        next_at[section_index] = present
            .get(idx + 1)
            .map(|(_, start)| *start)
            .unwrap_or(text.len());
    }

    let mut mistakes = Vec::new();
    let mut last_pos: Option<usize> = None;
    for (index, section) in REQUIRED_VIBE_SECTIONS.iter().enumerate() {
        match positions[index] {
            None => mistakes.push(VibeSectionMistake {
                kind: "missing_section",
                section,
            }),
            Some(pos) => {
                if last_pos.is_some_and(|prev| pos <= prev) {
                    mistakes.push(VibeSectionMistake {
                        kind: "out_of_order_section",
                        section,
                    });
                } else {
                    last_pos = Some(pos);
                }
                let body = &text[pos + section.len()..next_at[index]];
                if section_body_is_empty(body) {
                    mistakes.push(VibeSectionMistake {
                        kind: "empty_section",
                        section,
                    });
                }
            }
        }
    }
    mistakes
}

fn section_repair_issues(mistakes: &[VibeSectionMistake]) -> Vec<String> {
    let missing: Vec<&str> = mistakes
        .iter()
        .filter(|item| item.kind == "missing_section")
        .map(|item| item.section)
        .collect();
    let empty: Vec<&str> = mistakes
        .iter()
        .filter(|item| item.kind == "empty_section")
        .map(|item| item.section)
        .collect();
    let ordered: Vec<&str> = mistakes
        .iter()
        .filter(|item| item.kind == "out_of_order_section")
        .map(|item| item.section)
        .collect();
    let mut issues = Vec::new();
    if !missing.is_empty() {
        issues.push(format!(
            "Missing required template sections: {}. Add each missing section using [FILL: ...] when details are unknown.",
            missing.join(", ")
        ));
    }
    if !empty.is_empty() {
        issues.push(format!(
            "Empty template sections: {}. Put real content in each, using [FILL: ...] when details are unknown.",
            empty.join(", ")
        ));
    }
    if !ordered.is_empty() {
        issues.push(format!(
            "Template sections out of order: {}. Keep the canonical order from the constitution.",
            ordered.join(", ")
        ));
    }
    issues
}

fn missing_vibe_sections(text: &str) -> Vec<&'static str> {
    let mut seen = Vec::new();
    for mistake in vibe_section_mistakes(text) {
        if mistake.kind == "missing_section" && !seen.contains(&mistake.section) {
            seen.push(mistake.section);
        }
    }
    seen
}

fn dropped_proper_terms(source: &str, output: &str, dictionary: &[String]) -> Vec<String> {
    let haystack = output.to_lowercase();
    proper_terms(source, dictionary)
        .into_iter()
        .filter(|term| !haystack.contains(&term.to_lowercase()))
        .take(12)
        .collect()
}

fn proper_terms(source: &str, dictionary: &[String]) -> Vec<String> {
    let mut terms = dictionary
        .iter()
        .map(|term| term.trim())
        .filter(|term| term.len() > 1)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    for hotkey in ["fn", "fn+1", "fn+2", "fn+3", "Control+1", "Control+2"] {
        if source.contains(hotkey) {
            terms.push(hotkey.to_string());
        }
    }

    let mut current = Vec::new();
    for raw in source.split_whitespace() {
        let token = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if token.len() < 2 {
            flush_proper_term(&mut current, &mut terms);
            continue;
        }
        let starts_uppercase = token.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        let all_caps = token.chars().any(|c| c.is_ascii_alphabetic())
            && token
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_ascii_uppercase());
        if starts_uppercase || all_caps {
            current.push(token.to_string());
        } else {
            flush_proper_term(&mut current, &mut terms);
        }
    }
    flush_proper_term(&mut current, &mut terms);

    terms.sort_by_key(|term| term.to_lowercase());
    terms.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    terms
}

fn flush_proper_term(current: &mut Vec<String>, terms: &mut Vec<String>) {
    while current
        .first()
        .is_some_and(|term| !is_likely_named_single_token(term))
    {
        current.remove(0);
    }
    if current.is_empty() {
        return;
    }
    let joined = current.join(" ");
    let keep = current.len() > 1
        || current
            .first()
            .is_some_and(|term| is_likely_named_single_token(term));
    if keep && joined.len() > 2 {
        terms.push(joined);
    }
    current.clear();
}

fn is_likely_named_single_token(token: &str) -> bool {
    if token.chars().all(|c| !c.is_ascii_lowercase()) {
        return true;
    }
    !matches!(
        token,
        "Add"
            | "Build"
            | "Can"
            | "Change"
            | "Check"
            | "Clean"
            | "Create"
            | "Delete"
            | "Fix"
            | "Generate"
            | "Help"
            | "I"
            | "Make"
            | "Move"
            | "Remove"
            | "Run"
            | "Set"
            | "Show"
            | "Tell"
            | "Update"
            | "Use"
            | "Verify"
            | "What"
            | "When"
            | "Why"
    )
}

async fn answer_meeting_question(
    config: &AppConfig,
    question: &str,
    transcript: &str,
) -> Result<String, String> {
    let system = format!(
        r#"You help the user answer a question from a live conversation.

Use the transcript context to draft a concise answer the user can say or paste.

Rules:
- Answer only the detected question
- Be direct, natural, and useful in a live conversation
- Use facts from the transcript when relevant
- Do not invent private facts or commitments not supported by the conversation
- If the transcript does not contain enough information, give a safe answer that says what is known and what needs checking
- Return only the answer text, with no preamble, quotes, or explanation
- Language preference code: {lang}"#,
        lang = config.language,
    );

    let user = format!(
        "Detected question:\n{question}\n\n---\nRecent live conversation transcript:\n{transcript}\n\n---\nDraft the answer."
    );

    chat_completion(config, &system, &user, 900).await
}

async fn finish_dictation_text(
    app: &AppHandle,
    config: &AppConfig,
    raw_text: &str,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    let selected = take_selected_text(app).unwrap_or_default();
    let cleaned = if config.correct_english {
        let _ = app.emit("dictation-status", "correcting");
        light_cleanup_dictation(config, raw_text, store).await?
    } else {
        raw_text.to_string()
    };
    post_process_dictation(app, config, raw_text, &cleaned, &selected, store).await
}

async fn light_cleanup_dictation(
    config: &AppConfig,
    raw_text: &str,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    if cloud_api::uses_cloud_llm(config) {
        let dictionary = store.dictionary.iter().map(|d| d.word.clone()).collect();
        let payload = ProcessPayload {
            mode: "dictate",
            audio_base64: None,
            selected_text: Some(raw_text),
            project_context: None,
            constitution: None,
            skill: None,
            language: Some(config.language.as_str()),
            dictionary,
        };
        let processed = cloud_api::process(config, payload)
            .await
            .map_err(|err| format!("Light cleanup failed: {err}"))?;
        let text = processed.text.trim().to_string();
        if text.is_empty() {
            return Err("Light cleanup returned empty text.".into());
        }
        return Ok(text);
    }

    if !llm_key_configured(config) {
        return Err(
            "LLM API key missing. Add it in Settings, or set DEEPSEEK_API_KEY / XAI_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    let cleaned = light_cleanup_text(config, raw_text, store)
        .await
        .map_err(|err| format!("Light cleanup failed: {err}"))?;
    if !is_safe_light_cleanup(raw_text, &cleaned) {
        return Err("Light cleanup changed the transcript too much to trust.".into());
    }
    Ok(cleaned)
}

async fn post_process_dictation(
    app: &AppHandle,
    config: &AppConfig,
    raw_text: &str,
    cleaned: &str,
    selected: &str,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    if looks_like_edit_command(raw_text, selected) {
        let _ = app.emit("dictation-status", "correcting");
        match edit_selected_text(app, config, selected, raw_text).await {
            Ok(edited) if !edited.trim().is_empty() => return Ok(edited),
            Ok(_) => {}
            Err(err) => {
                let _ = app.emit(
                    "dictation-warning",
                    format!("Spoken edit failed, pasting dictation: {err}"),
                );
            }
        }
    }
    Ok(expand_snippets(cleaned, &store.snippets))
}

async fn light_cleanup_text(
    config: &AppConfig,
    text: &str,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    let dict = store
        .dictionary
        .iter()
        .map(|d| d.word.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let system = format!(
        r#"You clean up voice dictation into written text. Be conservative and surgical:
you are correcting transcription mechanics, not rewriting notes.

Rules:
- Preserve the user's words, order, meaning, note style, and level of detail
- Fix only clear transcription, spelling, punctuation, capitalization, and grammatical errors
- Remove filler words (um, uh, like, you know) and obvious stutters
- Honor mid-sentence take-backs: "meet at 5, actually 6" becomes "meet at 6"
- Keep fragments and rough notes as fragments; do not convert them into polished prose
- Do not summarize, shorten beyond take-backs, expand, rephrase, professionalize, or change tone
- Preserve negations exactly (not, no, never, don't, won't, without)
- Preserve numbers, dates, commands, file paths, code terms, product names, and proper nouns exactly
- If a word is a near-homophone of a dictionary term, replace it with the exact dictionary term
- Do not add greetings, explanations, or quotes
- If unsure whether a change is safe, leave that part unchanged
- Return only the corrected text
- Language preference code: {lang}
- Preserve these dictionary terms exactly when present: {dict}"#,
        lang = config.language,
        dict = if dict.is_empty() { "none" } else { &dict },
    );
    chat_completion(config, &system, text, output_cap_for_text(text, 400, 1600)).await
}

async fn edit_selected_text(
    app: &AppHandle,
    config: &AppConfig,
    selected: &str,
    instruction: &str,
) -> Result<String, String> {
    if cloud_api::uses_cloud_llm(config) {
        let store = load_store();
        let dictionary = store.dictionary.iter().map(|d| d.word.clone()).collect();
        let payload = ProcessPayload {
            mode: "edit_text",
            audio_base64: None,
            selected_text: Some(selected),
            project_context: Some(instruction),
            constitution: None,
            skill: None,
            language: Some(config.language.as_str()),
            dictionary,
        };
        let processed = cloud_api::process(config, payload).await?;
        let text = processed.text.trim().to_string();
        if text.is_empty() {
            return Err("Cloud edit returned empty text.".into());
        }
        return Ok(text);
    }
    if !llm_key_configured(config) {
        return Err("Spoken edit needs an LLM key.".into());
    }
    let system = format!(
        r#"You edit the selected text according to the user's spoken instruction.

Rules:
- Apply only the requested change
- Preserve meaning that the instruction does not ask to change
- Preserve proper nouns, numbers, paths, and dictionary terms
- Return only the edited text
- No preamble, quotes, or explanation
- Language preference code: {lang}"#,
        lang = config.language,
    );
    let user = format!(
        "Selected text:\n{selected}\n\n---\nSpoken instruction:\n{instruction}\n\n---\nReturn the edited text."
    );
    let _ = app;
    chat_completion(config, &system, &user, output_cap_for_text(selected, 400, 1600)).await
}

async fn polish_text(
    config: &AppConfig,
    text: &str,
    app_name: Option<&str>,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    let style = store
        .styles
        .iter()
        .find(|s| s.id == config.active_style_id)
        .map(|s| s.prompt.as_str())
        .unwrap_or("Write clear, natural prose.");

    let app_tone = if config.app_aware_tone {
        app_aware_instruction(app_name)
    } else {
        String::new()
    };

    let dict = store
        .dictionary
        .iter()
        .map(|d| d.word.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let system = format!(
        r#"You clean up voice dictation into written text.

Rules:
- Fix grammar, spelling, punctuation, and capitalization
- Remove filler words (um, uh, like, you know) unless meaningful
- Keep the speaker's meaning
- Do not add greetings, explanations, or quotes
- Return only the corrected text
- Language preference code: {lang}
- Style: {style}
- App tone guidance: {app_tone}
- Preserve these dictionary terms exactly when present: {dict}"#,
        lang = config.language,
        style = style,
        app_tone = if app_tone.is_empty() {
            "none"
        } else {
            &app_tone
        },
        dict = if dict.is_empty() { "none" } else { &dict },
    );

    chat_completion(config, &system, text, output_cap_for_text(text, 400, 1600)).await
}

fn app_aware_instruction(app_name: Option<&str>) -> String {
    let Some(name) = app_name.map(|s| s.to_lowercase()) else {
        return String::new();
    };

    if name.contains("slack") || name.contains("discord") || name.contains("messages") {
        "Match casual chat tone; keep it short.".into()
    } else if name.contains("mail") || name.contains("outlook") || name.contains("gmail") {
        "Match professional email tone.".into()
    } else if name.contains("cursor")
        || name.contains("code")
        || name.contains("terminal")
        || name.contains("iterm")
    {
        "Prefer precise technical wording suitable for coding tools.".into()
    } else if name.contains("notion") || name.contains("docs") || name.contains("word") {
        "Match clear document writing.".into()
    } else if name.contains("twitter") || name.contains("x") {
        "Keep it punchy and brief.".into()
    } else {
        format!("Adapt tone to the app: {name}.")
    }
}

async fn chat_completion(
    config: &AppConfig,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, String> {
    let base = config.llm_base_url.trim().trim_end_matches('/');
    let model = if config.llm_model.trim().is_empty() {
        "deepseek-v4-flash"
    } else {
        config.llm_model.trim()
    };

    let mut body = json!({
        "model": model,
        "temperature": 0.2,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });
    if config.llm_provider.trim().eq_ignore_ascii_case("deepseek") {
        body["thinking"] = json!({ "type": "disabled" });
    }

    let response = llm_client()
        .post(llm_url(base, "chat/completions"))
        .bearer_auth(config.llm_api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("LLM error ({status}): {body}"));
    }

    let parsed: ChatResponse = response
        .json()
        .await
        .map_err(|e| format!("LLM parse error: {e}"))?;

    let content = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default()
        .trim()
        .to_string();

    if content.is_empty() {
        return Err("LLM returned empty content.".into());
    }
    Ok(strip_model_wrapper(&content))
}

fn llm_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn output_cap_for_text(text: &str, floor: u32, ceiling: u32) -> u32 {
    // Roughly four chars per token, plus room for punctuation/formatting.
    let estimated = (text.chars().count() as u32 / 3).saturating_add(220);
    estimated.clamp(floor, ceiling)
}

fn strip_model_wrapper(text: &str) -> String {
    let mut t = text.trim().to_string();

    if t.starts_with("```") && t.ends_with("```") && t.len() > 6 {
        let mut inner = t[3..t.len() - 3].trim_matches('\n').to_string();
        if let Some(first_newline) = inner.find('\n') {
            if inner[..first_newline].split_whitespace().count() <= 1 {
                inner = inner[first_newline + 1..].to_string();
            }
        }
        t = inner.trim().to_string();
    }

    let mut lines = t.lines();
    if let Some(first) = lines.next() {
        let first_lower = first.trim().to_lowercase();
        if WRAPPER_PREAMBLE_PREFIXES
            .iter()
            .any(|prefix| first_lower.starts_with(prefix))
        {
            let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
            if !rest.is_empty() {
                t = rest;
            }
        }
    }

    if t.len() > 1 {
        let mut chars = t.chars();
        if let (Some(first), Some(last)) = (chars.next(), t.chars().last()) {
            if (first == '"' || first == '\'') && first == last {
                t = t[1..t.len() - 1].trim().to_string();
            }
        }
    }

    t.trim().to_string()
}

fn llm_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent("Flow.app/0.1 llm-client")
            .build()
            .expect("LLM HTTP client should build")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_missing_vibe_sections() {
        let missing = missing_vibe_sections("**Title**\n- Fix Flow\n\n**Task**\n- Do it");
        assert!(missing.contains(&"**Role & Stance**"));
        assert!(missing.contains(&"**Conflict Resolution**"));
        assert!(!missing.contains(&"**Title**"));
        assert!(!missing.contains(&"**Task**"));
    }

    #[test]
    fn detects_empty_and_out_of_order_vibe_sections() {
        let empty = concat!(
            "**Title**\n- Fix Flow\n\n",
            "**Role & Stance**\n- Engineer.\n\n",
            "**Task**\n- Do it.\n\n",
            "**Context**\n\n",
            "**Inputs Available**\n- Code.\n\n",
            "**Output Requirements**\n- A fix.\n\n",
            "**Constraints / Do-nots**\n- None.\n\n",
            "**Examples / References**\n- *None provided.*\n\n",
            "**Execution Checklist**\n- [ ] Do it.\n\n",
            "**Conflict Resolution**\n- Follow the task.\n"
        );
        let empty_kinds: Vec<_> = vibe_section_mistakes(empty)
            .into_iter()
            .map(|item| item.kind)
            .collect();
        assert!(empty_kinds.contains(&"empty_section"));

        let scrambled = concat!(
            "**Task**\n- Do it.\n\n",
            "**Title**\n- Fix Flow\n\n",
            "**Role & Stance**\n- Engineer.\n\n",
            "**Context**\n- App context.\n\n",
            "**Inputs Available**\n- Code.\n\n",
            "**Output Requirements**\n- A fix.\n\n",
            "**Constraints / Do-nots**\n- None.\n\n",
            "**Examples / References**\n- *None provided.*\n\n",
            "**Execution Checklist**\n- [ ] Do it.\n\n",
            "**Conflict Resolution**\n- Follow the task.\n"
        );
        let scrambled_kinds: Vec<_> = vibe_section_mistakes(scrambled)
            .into_iter()
            .map(|item| item.kind)
            .collect();
        assert!(scrambled_kinds.contains(&"out_of_order_section"));
        assert!(!scrambled_kinds.contains(&"missing_section"));
    }

    #[test]
    fn held_out_vibe_checker_cases() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../cloud/tests/fixtures/vibe_prompt_heldout.jsonl");
        let data = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let mut count = 0;
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let case: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|err| panic!("parse {line}: {err}"));
            let id = case["id"].as_str().unwrap();
            let source = case["source"].as_str().unwrap();
            let output = case["output"].as_str().unwrap();
            let dictionary: Vec<String> = case["dictionary"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect();
            let expected: std::collections::BTreeSet<&str> = case["expect_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();
            let mut kinds = std::collections::BTreeSet::new();
            for mistake in vibe_section_mistakes(output) {
                kinds.insert(mistake.kind);
            }
            if !dropped_proper_terms(source, output, &dictionary).is_empty() {
                kinds.insert("dropped_term");
            }
            assert_eq!(kinds, expected, "held-out case {id} kinds mismatch");
            count += 1;
        }
        assert_eq!(count, 30, "held-out fixture should stay at 30 labeled cases");
    }

    #[test]
    fn detects_dropped_proper_terms_without_common_leading_words() {
        let dictionary = vec!["Flow".to_string(), "fn+1".to_string()];
        let dropped = dropped_proper_terms(
            "Can you fix Flow local mode for Wispr Flow and fn+1",
            "Fix the local dictation mode and keep fn+1 working.",
            &dictionary,
        );
        assert!(dropped.iter().any(|term| term == "Flow"));
        assert!(dropped.iter().any(|term| term == "Wispr Flow"));
        assert!(!dropped.iter().any(|term| term == "Can"));
        assert!(!dropped.iter().any(|term| term == "fn+1"));
    }

    #[test]
    fn detects_dropped_fn_hotkeys() {
        let dropped = dropped_proper_terms(
            "Use fn for dictation, fn+1 for prompts, fn+2 for correction, and fn+3 for answers",
            "Use the function key for dictation and shortcuts for prompts/answers.",
            &[],
        );
        assert!(dropped.iter().any(|term| term == "fn"));
        assert!(dropped.iter().any(|term| term == "fn+1"));
        assert!(dropped.iter().any(|term| term == "fn+2"));
        assert!(dropped.iter().any(|term| term == "fn+3"));
    }

    #[test]
    fn raw_dictation_does_not_require_llm_but_correction_does() {
        // Raw dictation itself needs no LLM; `ensure_providers` adds the requirement
        // separately when `correct_english` is on, since cleanup can no longer degrade.
        assert!(!mode_requires_llm("dictate"));
        assert!(mode_requires_stt("dictate"));
        assert!(!mode_requires_stt("vibe_text"));
        assert!(mode_requires_llm("correct_text"));
        assert!(mode_requires_llm("meeting_answer"));
        assert!(mode_requires_llm("edit_text"));
        assert!(!mode_requires_stt("correct_text"));
        assert!(!mode_requires_stt("meeting_answer"));
    }

    fn test_config(mode: &str) -> AppConfig {
        AppConfig {
            processing_mode: mode.into(),
            stt_provider: "local_whisper".into(),
            groq_api_key: String::new(),
            llm_api_key: String::new(),
            flow_api_url: String::new(),
            flow_api_key: String::new(),
            llm_provider: "deepseek".into(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn local_mode_allows_dictate_without_cloud_or_llm_keys() {
        let mut config = test_config("local");
        // Cleanup has no raw-transcript fallback any more, so it is only keyless dictation
        // when the user has actually turned cleanup off.
        config.correct_english = false;
        assert!(ensure_providers(&config, "dictate").is_ok());
        assert!(ensure_providers(&config, "vibe_text").is_err());
    }

    #[test]
    fn dictation_with_cleanup_on_requires_an_llm_key() {
        let mut config = test_config("local");
        config.correct_english = true;
        assert!(ensure_providers(&config, "dictate").is_err());
    }

    #[test]
    fn hybrid_mode_needs_cloud_keys_but_not_a_local_llm_key() {
        let mut config = test_config("hybrid");
        assert!(ensure_providers(&config, "dictate").is_err());
        config.flow_api_url = "https://flow-api.example.run.app".into();
        config.flow_api_key = "test-key".into();
        assert!(ensure_providers(&config, "dictate").is_ok());
        assert!(ensure_providers(&config, "vibe_text").is_ok());
        assert!(ensure_providers(&config, "correct_text").is_ok());
    }

    #[test]
    fn cloud_mode_ignores_local_stt_and_llm_keys() {
        let mut config = test_config("cloud");
        config.stt_provider = "groq_whisper".into();
        assert!(ensure_providers(&config, "dictate").is_err());
        config.flow_api_url = "https://flow-api.example.run.app".into();
        config.flow_api_key = "test-key".into();
        assert!(ensure_providers(&config, "dictate").is_ok());
        assert!(ensure_providers(&config, "vibe_text").is_ok());
    }

    #[test]
    fn hybrid_groq_stt_still_needs_a_groq_key() {
        let mut config = test_config("hybrid");
        config.flow_api_url = "https://flow-api.example.run.app".into();
        config.flow_api_key = "test-key".into();
        config.stt_provider = "groq_whisper".into();
        assert!(ensure_providers(&config, "dictate").is_err());
        config.groq_api_key = "gsk_test".into();
        assert!(ensure_providers(&config, "dictate").is_ok());
    }

    #[test]
    fn strips_common_llm_wrappers_before_pasting() {
        let wrapped = "Here is your prompt:\n\n**Title**\n- Fix prompting";
        assert_eq!(strip_model_wrapper(wrapped), "**Title**\n- Fix prompting");

        let fenced = "```markdown\n**Title**\n- Fix prompting\n```";
        assert_eq!(strip_model_wrapper(fenced), "**Title**\n- Fix prompting");

        let quoted = "\"**Title**\n- Fix prompting\"";
        assert_eq!(strip_model_wrapper(quoted), "**Title**\n- Fix prompting");
    }
}
