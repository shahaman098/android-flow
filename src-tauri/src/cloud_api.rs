//! Mac client for Flow processing on Cloud Run.
//! All STT + LLM work happens in GCP; this module only uploads and receives text.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::config::AppConfig;
use crate::training::ProcessTrace;

#[derive(Debug, Deserialize)]
struct TextResponse {
    text: String,
    #[serde(default)]
    raw_transcript: Option<String>,
    #[serde(default)]
    trace: Option<ProcessTrace>,
}

#[derive(Debug)]
pub struct ProcessResult {
    pub text: String,
    pub raw_transcript: Option<String>,
    pub trace: ProcessTrace,
}

#[derive(Debug, Serialize)]
pub struct ProcessPayload<'a> {
    pub mode: &'a str,
    pub audio_base64: Option<&'a str>,
    pub selected_text: Option<&'a str>,
    pub project_context: Option<&'a str>,
    pub constitution: Option<&'a str>,
    pub skill: Option<&'a str>,
    pub language: Option<&'a str>,
    pub dictionary: Vec<String>,
}

fn ensure_cloud(config: &AppConfig) -> Result<(String, String), String> {
    let url = config.flow_api_url.trim().trim_end_matches('/').to_string();
    let key = config.flow_api_key.trim().to_string();
    if url.is_empty() {
        return Err(
            "FLOW_API_URL missing. Run cloud/deploy.sh, or switch Processing mode to local.".into(),
        );
    }
    if key.is_empty() {
        return Err(
            "FLOW_API_KEY missing. Re-run cloud/deploy.sh, or switch Processing mode to local."
                .into(),
        );
    }
    Ok((url, key))
}

fn cloud_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        // Cloud Run is healthy, but POST /v1/process is long-lived and more sensitive
        // to transport quirks than a tiny health probe. Keep the request alive longer
        // and prefer HTTP/1.1 over HTTP/2 stream behavior for stability.
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(120))
        .tcp_keepalive(Duration::from_secs(30))
        .http1_only()
        .user_agent("Flow.app/0.1 cloud-client")
        .build()
        .map_err(|e| format!("Could not build Cloud Run HTTP client: {e}"))
}

fn describe_network_error(context: &str, base: &str, err: &reqwest::Error) -> String {
    let kind = if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_request() {
        "request"
    } else if err.is_body() {
        "body"
    } else if err.is_decode() {
        "decode"
    } else {
        "transport"
    };

    format!(
        "{context} {kind} error for {base}. {err}. \
Check that Flow.app can reach the internet, then use Verify in Hub Settings. \
If /health works but /v1/process fails, the request path is unstable rather than the deployment being down."
    )
}

pub async fn health(config: &AppConfig) -> Result<String, String> {
    let (base, key) = ensure_cloud(config)?;
    let client = cloud_client()?;
    let response = client
        .get(format!("{base}/health"))
        .bearer_auth(&key)
        .send()
        .await
        .map_err(|e| describe_network_error("Cloud Run", &base, &e))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Cloud Run health failed ({status}): {body}"));
    }
    let body = response.text().await.unwrap_or_default();
    Ok(format!("Flow Cloud API OK — {body}"))
}

pub async fn transcribe(config: &AppConfig, audio_base64: &str) -> Result<String, String> {
    let (base, key) = ensure_cloud(config)?;
    let client = cloud_client()?;
    let response = client
        .post(format!("{base}/v1/transcribe"))
        .bearer_auth(&key)
        .json(&json!({
            "audio_base64": audio_base64,
            "language": config.language,
            "dictionary": crate::store::load_store()
                .dictionary
                .iter()
                .map(|entry| entry.word.clone())
                .collect::<Vec<_>>(),
        }))
        .send()
        .await
        .map_err(|e| describe_network_error("Cloud STT", &format!("{base}/v1/transcribe"), &e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Cloud STT error ({status}): {body}"));
    }

    let parsed: TextResponse = response
        .json()
        .await
        .map_err(|e| format!("Cloud STT parse error: {e}"))?;
    if parsed.text.trim().is_empty() {
        return Err("Cloud STT returned empty text.".into());
    }
    Ok(parsed.text)
}

pub async fn process(
    config: &AppConfig,
    payload: ProcessPayload<'_>,
) -> Result<ProcessResult, String> {
    let (base, key) = ensure_cloud(config)?;
    let client = cloud_client()?;
    let response = client
        .post(format!("{base}/v1/process"))
        .bearer_auth(&key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| describe_network_error("Cloud process", &format!("{base}/v1/process"), &e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Cloud process error ({status}): {body}"));
    }

    let parsed: TextResponse = response
        .json()
        .await
        .map_err(|e| format!("Cloud process parse error: {e}"))?;
    if parsed.text.trim().is_empty() {
        return Err("Cloud process returned empty text.".into());
    }
    Ok(ProcessResult {
        text: parsed.text,
        raw_transcript: parsed.raw_transcript,
        trace: parsed.trace.unwrap_or_default(),
    })
}

/// Transcribes one meeting-audio chunk. Reuses `TextResponse` — `raw_transcript` is simply left
/// `None` since the meeting endpoint doesn't return it, and the extra `speaker` field on the
/// response is ignored (serde ignores unknown fields by default).
pub async fn meeting_transcribe(
    config: &AppConfig,
    audio_base64: &str,
    speaker: &str,
    session_id: &str,
) -> Result<String, String> {
    let (base, key) = ensure_cloud(config)?;
    let client = cloud_client()?;
    let response = client
        .post(format!("{base}/v1/meeting/transcribe"))
        .bearer_auth(&key)
        .json(&json!({
            "audio_base64": audio_base64,
            "speaker": speaker,
            "session_id": session_id,
            "language": config.language,
            "dictionary": crate::store::load_store()
                .dictionary
                .iter()
                .map(|entry| entry.word.clone())
                .collect::<Vec<_>>(),
        }))
        .send()
        .await
        .map_err(|e| {
            describe_network_error(
                "Meeting transcribe",
                &format!("{base}/v1/meeting/transcribe"),
                &e,
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Meeting transcribe error ({status}): {body}"));
    }

    let parsed: TextResponse = response
        .json()
        .await
        .map_err(|e| format!("Meeting transcribe parse error: {e}"))?;
    if parsed.text.trim().is_empty() {
        return Err("Meeting transcribe returned empty text.".into());
    }
    Ok(parsed.text)
}

pub fn processing_mode(config: &AppConfig) -> &'static str {
    match config.processing_mode.trim().to_ascii_lowercase().as_str() {
        "cloud" => "cloud",
        "hybrid" => "hybrid",
        _ => "local",
    }
}

/// Full Cloud Run path: STT and LLM both run on `flow-api`.
pub fn uses_cloud(config: &AppConfig) -> bool {
    processing_mode(config) == "cloud"
}

/// Cloud or hybrid: cleanup / prompts / answers run on Cloud Run.
pub fn uses_cloud_llm(config: &AppConfig) -> bool {
    matches!(processing_mode(config), "cloud" | "hybrid")
}

/// Local or hybrid: speech is transcribed on the Mac.
pub fn uses_local_stt(config: &AppConfig) -> bool {
    matches!(processing_mode(config), "local" | "hybrid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn cfg(mode: &str) -> AppConfig {
        AppConfig {
            processing_mode: mode.into(),
            ..AppConfig::default()
        }
    }

    #[test]
    fn local_mode_keeps_stt_and_llm_on_the_mac() {
        let config = cfg("local");
        assert_eq!(processing_mode(&config), "local");
        assert!(!uses_cloud(&config));
        assert!(!uses_cloud_llm(&config));
        assert!(uses_local_stt(&config));
    }

    #[test]
    fn cloud_mode_sends_stt_and_llm_to_cloud_run() {
        let config = cfg("CLOUD");
        assert_eq!(processing_mode(&config), "cloud");
        assert!(uses_cloud(&config));
        assert!(uses_cloud_llm(&config));
        assert!(!uses_local_stt(&config));
    }

    #[test]
    fn hybrid_mode_is_local_stt_plus_cloud_llm() {
        let config = cfg("hybrid");
        assert_eq!(processing_mode(&config), "hybrid");
        assert!(!uses_cloud(&config));
        assert!(uses_cloud_llm(&config));
        assert!(uses_local_stt(&config));
    }
}
