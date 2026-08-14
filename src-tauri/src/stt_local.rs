use crate::config::AppConfig;
use crate::{stt_gcp, stt_groq, stt_whisper};

pub async fn transcribe(config: &AppConfig, audio_bytes: Vec<u8>) -> Result<String, String> {
    match config.stt_provider.trim() {
        "" | "local" | "local_whisper" | "whisper_cpp" => {
            stt_whisper::transcribe(config, audio_bytes).await
        }
        "groq" | "groq_whisper" => stt_groq::transcribe(config, audio_bytes).await,
        "gcp_speech" => stt_gcp::transcribe(config, audio_bytes).await,
        other => Err(format!(
            "Unsupported STT provider '{other}'. Choose local_whisper, groq_whisper, or gcp_speech in Settings."
        )),
    }
}

pub async fn verify(config: &AppConfig) -> Result<String, String> {
    match config.stt_provider.trim() {
        "" | "local" | "local_whisper" | "whisper_cpp" => stt_whisper::verify(config),
        "groq" | "groq_whisper" => stt_groq::verify(config).await,
        "gcp_speech" => stt_gcp::verify_speech(config).await,
        other => Err(format!(
            "Unsupported STT provider '{other}'. Choose local_whisper, groq_whisper, or gcp_speech in Settings."
        )),
    }
}

pub fn readiness_gaps(config: &AppConfig) -> Vec<String> {
    match config.stt_provider.trim() {
        "" | "local" | "local_whisper" | "whisper_cpp" => stt_whisper::readiness_gaps(config),
        "groq" | "groq_whisper" => {
            if config.groq_api_key.trim().is_empty() {
                vec![
                    "Add Groq API key in Settings, or GROQ_API_KEY in Application Support/voice-flow/.env."
                        .into(),
                ]
            } else {
                Vec::new()
            }
        }
        "gcp_speech" => {
            let mut gaps = Vec::new();
            if config.gcp_project_id.trim().is_empty() {
                gaps.push("Set GCP project ID.".into());
            }
            if let Err(err) = stt_gcp::adc_access_token() {
                gaps.push(err);
            }
            gaps
        }
        other => vec![format!(
            "Unsupported STT provider '{other}'. Choose local_whisper, groq_whisper, or gcp_speech in Settings."
        )],
    }
}
