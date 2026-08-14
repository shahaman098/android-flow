use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::config::data_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: String,
    pub word: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub trigger: String,
    pub expansion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub text: String,
    pub app_name: Option<String>,
    pub created_at: String,
    pub word_count: usize,
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stats {
    pub total_dictations: u64,
    pub total_words: u64,
    #[serde(default)]
    pub total_prompts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// "you" (mic) or "other" (system audio).
    pub speaker: String,
    pub text: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingTranscript {
    pub id: String,
    pub app_name: String,
    pub started_at: String,
    pub ended_at: String,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlowStore {
    pub dictionary: Vec<DictionaryEntry>,
    pub snippets: Vec<Snippet>,
    pub styles: Vec<Style>,
    pub history: Vec<HistoryItem>,
    pub stats: Stats,
    #[serde(default)]
    pub meetings: Vec<MeetingTranscript>,
}

fn store_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("store.json"))
}

fn default_styles() -> Vec<Style> {
    vec![
        Style {
            id: "clear".into(),
            name: "Clear".into(),
            prompt: "Write clear, natural prose. Keep the meaning.".into(),
        },
        Style {
            id: "professional".into(),
            name: "Professional".into(),
            prompt: "Use a polished, professional tone suitable for work email.".into(),
        },
        Style {
            id: "casual".into(),
            name: "Casual".into(),
            prompt: "Use a friendly, casual tone suitable for chat.".into(),
        },
        Style {
            id: "concise".into(),
            name: "Concise".into(),
            prompt: "Make the text concise and direct. Remove fluff.".into(),
        },
    ]
}

pub fn load_store() -> FlowStore {
    let path = match store_path() {
        Ok(p) => p,
        Err(_) => {
            return FlowStore {
                styles: default_styles(),
                ..Default::default()
            };
        }
    };

    if !path.exists() {
        let store = FlowStore {
            styles: default_styles(),
            ..Default::default()
        };
        let _ = save_store(&store);
        return store;
    }

    match fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<FlowStore>(&raw) {
            Ok(mut store) => {
                if store.styles.is_empty() {
                    store.styles = default_styles();
                }
                store
            }
            Err(_) => {
                crate::config::quarantine_corrupt(&path);
                FlowStore {
                    styles: default_styles(),
                    ..Default::default()
                }
            }
        },
        Err(_) => FlowStore {
            styles: default_styles(),
            ..Default::default()
        },
    }
}

pub fn save_store(store: &FlowStore) -> Result<(), String> {
    let path = store_path()?;
    let raw = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    crate::config::atomic_write_private(&path, &raw)
}

pub fn add_history(text: &str, app_name: Option<String>, mode: &str) -> Result<(), String> {
    let mut store = load_store();
    let word_count = text.split_whitespace().count();
    store.history.insert(
        0,
        HistoryItem {
            id: Uuid::new_v4().to_string(),
            text: text.to_string(),
            app_name,
            created_at: chrono_like_now(),
            word_count,
            mode: mode.to_string(),
        },
    );
    store.history.truncate(200);
    store.stats.total_words += word_count as u64;
    if mode == "vibe_text" {
        store.stats.total_prompts += 1;
    } else {
        store.stats.total_dictations += 1;
    }
    save_store(&store)
}

pub fn add_meeting_transcript(transcript: MeetingTranscript) -> Result<(), String> {
    let mut store = load_store();
    store.meetings.insert(0, transcript);
    store.meetings.truncate(100);
    save_store(&store)
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

#[tauri::command]
pub fn get_store() -> Result<FlowStore, String> {
    Ok(load_store())
}

pub fn learn_dictionary_from_correction(model_output: &str, gold: &str) {
    for word in crate::dictation_post::learnable_dictionary_terms(model_output, gold) {
        let _ = add_dictionary_word(word);
    }
}

#[tauri::command]
pub fn add_dictionary_word(word: String) -> Result<FlowStore, String> {
    let word = word.trim().to_string();
    if word.is_empty() {
        return Err("Word cannot be empty".into());
    }
    let mut store = load_store();
    if store
        .dictionary
        .iter()
        .any(|e| e.word.eq_ignore_ascii_case(&word))
    {
        return Ok(store);
    }
    store.dictionary.push(DictionaryEntry {
        id: Uuid::new_v4().to_string(),
        word,
    });
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn remove_dictionary_word(id: String) -> Result<FlowStore, String> {
    let mut store = load_store();
    store.dictionary.retain(|e| e.id != id);
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn add_snippet(trigger: String, expansion: String) -> Result<FlowStore, String> {
    let trigger = trigger.trim().to_string();
    let expansion = expansion.to_string();
    if trigger.is_empty() || expansion.trim().is_empty() {
        return Err("Trigger and expansion are required".into());
    }
    let mut store = load_store();
    store.snippets.push(Snippet {
        id: Uuid::new_v4().to_string(),
        trigger,
        expansion,
    });
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn remove_snippet(id: String) -> Result<FlowStore, String> {
    let mut store = load_store();
    store.snippets.retain(|s| s.id != id);
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn upsert_style(style: Style) -> Result<FlowStore, String> {
    let mut store = load_store();
    if let Some(existing) = store.styles.iter_mut().find(|s| s.id == style.id) {
        *existing = style;
    } else {
        store.styles.push(style);
    }
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn clear_history() -> Result<FlowStore, String> {
    let mut store = load_store();
    store.history.clear();
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn remove_meeting_transcript(id: String) -> Result<FlowStore, String> {
    let mut store = load_store();
    store.meetings.retain(|m| m.id != id);
    save_store(&store)?;
    Ok(store)
}

#[tauri::command]
pub fn clear_meetings() -> Result<FlowStore, String> {
    let mut store = load_store();
    store.meetings.clear();
    save_store(&store)?;
    Ok(store)
}
