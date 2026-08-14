use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::config::{data_dir, AppConfig};
use crate::dictation_post::{is_unusable_stt, sanitize_stt_transcript};
use crate::store::load_store;
use crate::stt_whisper_server;

const LOCAL_MODEL: &str = "ggml-small.en.bin";
pub(crate) const WHISPER_THREADS: u32 = 4;

pub async fn transcribe(config: &AppConfig, audio_bytes: Vec<u8>) -> Result<String, String> {
    if audio_bytes.len() < 1500 {
        return Err(format!(
            "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
            audio_bytes.len()
        ));
    }

    let model_path = resolve_model_path(config)?;
    if !model_path.exists() {
        return Err(format!(
            "Local Whisper model missing at {}. Download {LOCAL_MODEL} into voice-flow/models/.",
            model_path.display()
        ));
    }

    // The resident server is the only local engine. Nothing catches a failure here, so wait
    // out a cold model load rather than reporting an error the user cannot act on.
    let base = stt_whisper_server::endpoint(&model_path, WHISPER_THREADS, true).ok_or_else(
        || {
            "Local Whisper server did not start. Check `brew install whisper-cpp ffmpeg`, then restart Flow."
                .to_string()
        },
    )?;
    transcribe_via_server(&base, config, audio_bytes).await
}

pub fn verify(config: &AppConfig) -> Result<String, String> {
    let server = resolve_executable("whisper-server").ok_or_else(|| {
        "whisper-server not found. Install with: brew install whisper-cpp".to_string()
    })?;
    let ffmpeg = resolve_executable("ffmpeg")
        .ok_or_else(|| "ffmpeg not found. Install with: brew install ffmpeg".to_string())?;
    let model = resolve_model_path(config)?;
    if !model.exists() {
        return Err(format!(
            "Local Whisper model missing at {}. Download {LOCAL_MODEL} into voice-flow/models/.",
            model.display()
        ));
    }
    Ok(format!(
        "Local Whisper OK — {} with model {} and {}.",
        server.display(),
        model.display(),
        ffmpeg.display()
    ))
}

pub fn readiness_gaps(config: &AppConfig) -> Vec<String> {
    let mut gaps = Vec::new();
    if resolve_executable("whisper-server").is_none() {
        gaps.push("Install local Whisper: brew install whisper-cpp.".into());
    }
    // The server shells out to ffmpeg for its `--convert` transcode.
    if resolve_executable("ffmpeg").is_none() {
        gaps.push("Install ffmpeg for audio conversion: brew install ffmpeg.".into());
    }
    match resolve_model_path(config) {
        Ok(model) if model.exists() => {}
        Ok(model) => gaps.push(format!(
            "Download local Whisper model to {}.",
            model.display()
        )),
        Err(err) => gaps.push(err),
    }
    gaps
}

/// Transcribe through the resident server — the only local path.
async fn transcribe_via_server(
    base: &str,
    config: &AppConfig,
    audio_bytes: Vec<u8>,
) -> Result<String, String> {
    let mut form = reqwest::multipart::Form::new()
        .text("response_format", "text")
        .text("temperature", "0");

    let language = config.language.trim();
    if !language.is_empty() && language != "auto" {
        form = form.text("language", language.to_string());
    }
    let prompt = local_prompt();
    if !prompt.is_empty() {
        form = form.text("prompt", prompt);
    }

    // The server transcodes via `--convert`, so the browser's webm goes straight through.
    let part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name("flow-dictation.webm")
        .mime_str("audio/webm")
        .map_err(|e| format!("Could not prepare local Whisper upload: {e}"))?;
    form = form.part("file", part);

    let response = server_client()
        .post(format!("{base}/inference"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Local Whisper server request failed: {e}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Local Whisper server error ({status}): {body}"));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Local Whisper server read failed: {e}"))?;
    let text = sanitize_stt_transcript(
        &body
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    );
    if text.is_empty() || is_unusable_stt(&text) {
        return Err("Local Whisper returned empty text (no speech detected).".into());
    }
    Ok(text)
}

/// Reused across requests — building a client per call re-does TLS/pool setup for nothing.
fn server_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    })
}

/// Model to preload at startup — only when local Whisper is the configured engine and the
/// weights are actually present, so a Groq/GCP user never pays for a resident server.
pub(crate) fn prewarm_model_path(config: &AppConfig) -> Option<PathBuf> {
    if !matches!(
        config.stt_provider.trim(),
        "" | "local" | "local_whisper" | "whisper_cpp"
    ) {
        return None;
    }
    let model = resolve_model_path(config).ok()?;
    model.exists().then_some(model)
}

fn resolve_model_path(config: &AppConfig) -> Result<PathBuf, String> {
    let configured = config.local_whisper_model_path.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    Ok(data_dir()?.join("models").join(LOCAL_MODEL))
}

fn local_prompt() -> String {
    let mut terms = vec![
        "Wispr Flow".to_string(),
        "Flow".to_string(),
        "Vibe Coding".to_string(),
        "Codex".to_string(),
        "Tauri".to_string(),
        "Whisper".to_string(),
        "DeepSeek".to_string(),
        "Groq".to_string(),
        "GCP".to_string(),
        "fn".to_string(),
        "fn+1".to_string(),
        "fn+2".to_string(),
    ];

    let store = load_store();
    terms.extend(
        store
            .dictionary
            .iter()
            .map(|entry| entry.word.trim())
            .filter(|word| !word.is_empty())
            .map(ToOwned::to_owned),
    );

    terms.sort_by_key(|term| term.to_lowercase());
    terms.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    terms.truncate(80);

    terms.join(", ")
}

pub(crate) fn resolve_executable(name: &str) -> Option<PathBuf> {
    let candidates = [
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        name.to_string(),
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| executable_exists(path))
}

fn executable_exists(path: &Path) -> bool {
    if path.components().count() > 1 {
        path.exists()
    } else {
        Command::new(path)
            .arg("-version")
            .output()
            .or_else(|_| Command::new(path).arg("--version").output())
            .is_ok()
    }
}




#[cfg(test)]
mod tests {
    use super::*;

    fn local_config(provider: &str) -> AppConfig {
        AppConfig {
            stt_provider: provider.into(),
            // Point at a path that cannot exist so the check under test is the provider gate.
            local_whisper_model_path: "/nonexistent/flow-test-model.bin".into(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn a_cloud_stt_user_never_starts_a_resident_whisper_server() {
        for provider in ["groq", "groq_whisper", "gcp_speech"] {
            assert!(
                prewarm_model_path(&local_config(provider)).is_none(),
                "{provider} should not preload local Whisper weights"
            );
        }
    }

    #[test]
    fn a_missing_model_is_not_offered_for_prewarming() {
        for provider in ["", "local", "local_whisper", "whisper_cpp"] {
            assert!(
                prewarm_model_path(&local_config(provider)).is_none(),
                "{provider} must not prewarm a model that is not on disk"
            );
        }
    }
}
