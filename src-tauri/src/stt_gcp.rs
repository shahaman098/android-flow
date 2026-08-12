use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::{language_bcp47, AppConfig};

#[derive(Debug, Deserialize)]
struct RecognizeResponse {
    results: Option<Vec<RecognizeResult>>,
}

#[derive(Debug, Deserialize)]
struct RecognizeResult {
    alternatives: Option<Vec<RecognizeAlternative>>,
}

#[derive(Debug, Deserialize)]
struct RecognizeAlternative {
    transcript: Option<String>,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

fn token_cache() -> &'static Mutex<Option<CachedToken>> {
    static CACHE: OnceLock<Mutex<Option<CachedToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Fetch a Google OAuth access token from Application Default Credentials.
pub fn adc_access_token() -> Result<String, String> {
    if let Ok(guard) = token_cache().lock() {
        if let Some(cached) = guard.as_ref() {
            if cached.expires_at > Instant::now() {
                return Ok(cached.value.clone());
            }
        }
    }

    let output = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .map_err(|e| {
            format!(
                "Could not run gcloud for ADC token ({e}). Install Google Cloud SDK and run: gcloud auth application-default login"
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ADC missing or expired. Run: gcloud auth application-default login (account sahkris0844@gmail.com). Details: {stderr}"
        ));
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err("gcloud returned an empty ADC access token.".into());
    }

    if let Ok(mut guard) = token_cache().lock() {
        *guard = Some(CachedToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(50 * 60),
        });
    }

    Ok(token)
}

pub async fn check_adc() -> Result<String, String> {
    let token = adc_access_token()?;
    let preview = if token.len() > 12 {
        format!("{}…", &token[..8])
    } else {
        "ok".into()
    };
    Ok(format!("ADC OK (token {preview})"))
}

pub async fn transcribe(config: &AppConfig, audio_bytes: Vec<u8>) -> Result<String, String> {
    if config.stt_provider != "gcp_speech" {
        return Err(format!(
            "Unsupported STT provider '{}'. Expected gcp_speech.",
            config.stt_provider
        ));
    }

    let project = config.gcp_project_id.trim();
    let location = config.gcp_location.trim();
    if project.is_empty() || location.is_empty() {
        return Err("GCP project ID and location are required for Speech-to-Text.".into());
    }

    let token = adc_access_token()?;
    let language = language_bcp47(&config.language);
    let model = if config.stt_model.trim().is_empty() {
        "latest_long"
    } else {
        config.stt_model.trim()
    };

    let content = STANDARD.encode(&audio_bytes);
    let url = format!(
        "https://{location}-speech.googleapis.com/v2/projects/{project}/locations/{location}/recognizers/_:recognize"
    );

    let body = json!({
        "config": {
            "autoDecodingConfig": {},
            "languageCodes": [language],
            "model": model
        },
        "content": content
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("GCP Speech network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 403 || body.contains("SERVICE_DISABLED") {
            return Err(format!(
                "GCP Speech API error ({status}). Enable speech.googleapis.com on project {project}. Details: {body}"
            ));
        }
        if status.as_u16() == 401 {
            return Err(format!(
                "GCP Speech auth failed ({status}). Re-run: gcloud auth application-default login. Details: {body}"
            ));
        }
        return Err(format!("GCP Speech error ({status}): {body}"));
    }

    let parsed: RecognizeResponse = response
        .json()
        .await
        .map_err(|e| format!("GCP Speech parse error: {e}"))?;

    let mut parts = Vec::new();
    for result in parsed.results.unwrap_or_default() {
        if let Some(alts) = result.alternatives {
            if let Some(first) = alts.into_iter().next() {
                if let Some(t) = first.transcript {
                    let t = t.trim();
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                }
            }
        }
    }

    let text = parts.join(" ").trim().to_string();
    if text.is_empty() {
        return Err("GCP Speech returned empty text (no speech detected in audio).".into());
    }
    Ok(text)
}

pub async fn verify_speech(config: &AppConfig) -> Result<String, String> {
    let _ = check_adc().await?;
    let project = config.gcp_project_id.trim();
    let location = config.gcp_location.trim();
    let token = adc_access_token()?;

    // Lightweight metadata call — list recognizers (or empty recognizer probe).
    let url = format!(
        "https://{location}-speech.googleapis.com/v2/projects/{project}/locations/{location}/recognizers?pageSize=1"
    );
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("GCP Speech verify network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "GCP Speech verify failed ({status}) for project {project} / {location}: {body}"
        ));
    }

    Ok(format!(
        "GCP Speech OK — project {project}, region {location}, model {}.",
        if config.stt_model.trim().is_empty() {
            "latest_long"
        } else {
            config.stt_model.trim()
        }
    ))
}
