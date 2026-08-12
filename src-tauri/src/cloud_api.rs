//! Mac client for Flow processing on MyGCP Cloud Run.
//! All STT + LLM work happens in GCP; this module only uploads and receives text.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::config::AppConfig;

#[derive(Debug, Deserialize)]
struct TextResponse {
    text: String,
    #[serde(default)]
    raw_transcript: Option<String>,
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
            "FLOW_API_URL missing. Deploy cloud/ to MyGCP and set FLOW_API_URL in voice-flow/.env."
                .into(),
        );
    }
    if key.is_empty() {
        return Err(
            "FLOW_API_KEY missing. Import from Secret Manager flow-api-key or re-run cloud/deploy.sh."
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
        .timeout(Duration::from_secs(540))
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
    // health is public; key optional — still check reachability
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Cloud Run health failed ({status}): {body}"));
    }
    let body = response.text().await.unwrap_or_default();
    Ok(format!("MyGCP Flow API OK — {body}"))
}

pub async fn transcribe(
    config: &AppConfig,
    audio_base64: &str,
) -> Result<String, String> {
    let (base, key) = ensure_cloud(config)?;
    let client = cloud_client()?;
    let response = client
        .post(format!("{base}/v1/transcribe"))
        .bearer_auth(&key)
        .json(&json!({
            "audio_base64": audio_base64,
            "language": config.language,
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
) -> Result<(String, Option<String>), String> {
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
    Ok((parsed.text, parsed.raw_transcript))
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

pub fn uses_cloud(config: &AppConfig) -> bool {
    config.processing_mode.trim().eq_ignore_ascii_case("cloud")
}
