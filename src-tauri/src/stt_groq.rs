use serde::Deserialize;
use std::time::Duration;

use crate::config::AppConfig;
use crate::dictation_post::{is_unusable_stt, sanitize_stt_transcript};
use crate::store::load_store;

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
}

pub async fn transcribe(config: &AppConfig, audio_bytes: Vec<u8>) -> Result<String, String> {
    let key = config.groq_api_key.trim();
    if key.is_empty() {
        return Err(
            "Groq API key missing. Add it in Settings, or set GROQ_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    if audio_bytes.len() < 1500 {
        return Err(format!(
            "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
            audio_bytes.len()
        ));
    }

    let model = if config.groq_stt_model.trim().is_empty() {
        "whisper-large-v3-turbo"
    } else {
        config.groq_stt_model.trim()
    };

    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "json")
        .text("temperature", "0");

    let language = config.language.trim();
    if !language.is_empty() && language != "auto" {
        form = form.text("language", language.to_string());
    }
    if let Some(prompt) = groq_dictionary_prompt() {
        form = form.text("prompt", prompt);
    }

    let file_part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name("flow-dictation.webm")
        .mime_str("audio/webm")
        .map_err(|e| format!("Could not prepare Groq audio upload: {e}"))?;
    form = form.part("file", file_part);

    let response = groq_client()
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Groq Speech network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Groq Speech error ({status}): {body}"));
    }

    let parsed: TranscriptionResponse = response
        .json()
        .await
        .map_err(|e| format!("Groq Speech parse error: {e}"))?;
    let text = sanitize_stt_transcript(&parsed.text);
    if text.is_empty() || is_unusable_stt(&text) {
        return Err("Groq Speech returned empty text (no speech detected).".into());
    }
    Ok(text)
}

pub async fn verify(config: &AppConfig) -> Result<String, String> {
    let key = config.groq_api_key.trim();
    if key.is_empty() {
        return Err(
            "Groq API key missing. Add it in Settings, or set GROQ_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    let response = groq_client()
        .get("https://api.groq.com/openai/v1/models")
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| format!("Groq verify network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Groq auth/API error ({status}): {body}"));
    }

    Ok(format!(
        "Groq Speech OK — model {}.",
        if config.groq_stt_model.trim().is_empty() {
            "whisper-large-v3-turbo"
        } else {
            config.groq_stt_model.trim()
        }
    ))
}

pub(crate) fn groq_dictionary_prompt() -> Option<String> {
    let words: Vec<String> = load_store()
        .dictionary
        .iter()
        .map(|entry| entry.word.trim())
        .filter(|word| !word.is_empty())
        .map(ToOwned::to_owned)
        .take(60)
        .collect();
    format_groq_dictionary_prompt(&words)
}

pub(crate) fn format_groq_dictionary_prompt(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    Some(words.join(", ").chars().take(900).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_groq_dictionary_prompt() {
        let prompt = format_groq_dictionary_prompt(&[
            "Flow".to_string(),
            "Wispr".to_string(),
        ]);
        assert_eq!(prompt.as_deref(), Some("Flow, Wispr"));
        assert!(format_groq_dictionary_prompt(&[]).is_none());
    }
}

fn groq_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .user_agent("Flow.app/0.1 groq-speech-client")
            .build()
            .expect("Groq Speech HTTP client should build")
    })
}
