//! Consent-gated local meeting transcription.
//!
//! Detects known call apps running on the Mac and surfaces that to the Hub, but detection alone
//! never starts capture. Capture only starts after `meeting_confirm_notified` has been called for
//! the current session — `meeting_start_capture` refuses to run otherwise. Only transcript text,
//! speaker labels, and timestamps are ever persisted; raw audio bytes exist only transiently in
//! memory between capture and the STT request.

use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::cloud_api;
use crate::config::load_config;
use crate::store::{self, MeetingTranscript, TranscriptSegment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPhase {
    Idle,
    Detected,
    Notified,
    Capturing,
}

pub struct MeetingSessionInner {
    pub phase: MeetingPhase,
    pub session_id: Option<String>,
    pub app_name: Option<String>,
    pub started_at: Option<Instant>,
    pub started_at_wall: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    #[cfg(target_os = "macos")]
    pub system_audio: Option<crate::system_audio::SystemAudioHandle>,
}

impl Default for MeetingSessionInner {
    fn default() -> Self {
        Self {
            phase: MeetingPhase::Idle,
            session_id: None,
            app_name: None,
            started_at: None,
            started_at_wall: None,
            segments: Vec::new(),
            #[cfg(target_os = "macos")]
            system_audio: None,
        }
    }
}

#[derive(Default)]
pub struct MeetingSession {
    pub inner: Mutex<MeetingSessionInner>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingStatus {
    pub phase: MeetingPhase,
    pub app_name: Option<String>,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MeetingQuestionContext {
    pub question: String,
    pub transcript: String,
}

const KNOWN_APPS: &[(&str, &str)] = &[
    ("zoom.us", "Zoom"),
    ("MSTeams", "Microsoft Teams"),
    ("Microsoft Teams", "Microsoft Teams"),
    ("Slack Helper", "Slack"),
    ("Slack", "Slack"),
    ("Discord", "Discord"),
    ("WhatsApp", "WhatsApp"),
    ("FaceTime", "FaceTime"),
];

fn now_epoch_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

fn status_from(inner: &MeetingSessionInner) -> MeetingStatus {
    MeetingStatus {
        phase: inner.phase,
        app_name: inner.app_name.clone(),
        started_at: inner.started_at_wall.clone(),
    }
}

fn emit_status(app: &AppHandle, inner: &MeetingSessionInner) {
    let _ = app.emit("meeting-phase-changed", status_from(inner));
}

#[tauri::command]
pub fn meeting_get_status(app: AppHandle) -> Result<MeetingStatus, String> {
    let state = app.state::<MeetingSession>();
    let inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;
    Ok(status_from(&inner))
}

/// Pure state transition — no audio side effects. Must be called before `meeting_start_capture`
/// will accept a session.
#[tauri::command]
pub fn meeting_confirm_notified(app: AppHandle) -> Result<MeetingStatus, String> {
    let state = app.state::<MeetingSession>();
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;

    if !matches!(inner.phase, MeetingPhase::Detected | MeetingPhase::Notified) {
        return Err("No detected call to confirm notification for.".into());
    }

    inner.phase = MeetingPhase::Notified;
    inner.session_id = Some(Uuid::new_v4().to_string());
    emit_status(&app, &inner);
    Ok(status_from(&inner))
}

/// The enforcement point: refuses to start capturing audio unless the current session has an
/// explicit `Notified` confirmation. System-audio capture (ScreenCaptureKit) is wired in
/// separately once available; this only guarantees the mic path and the consent gate itself.
#[tauri::command]
pub fn meeting_start_capture(app: AppHandle) -> Result<MeetingStatus, String> {
    let state = app.state::<MeetingSession>();
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;

    if inner.phase != MeetingPhase::Notified {
        return Err(
            "Cannot start capture: participants have not been notified for this session.".into(),
        );
    }

    inner.phase = MeetingPhase::Capturing;
    inner.started_at = Some(Instant::now());
    inner.started_at_wall = Some(now_epoch_secs());
    inner.segments.clear();

    #[cfg(target_os = "macos")]
    {
        // No mic-only degradation: a capture without system audio is not the session the
        // user asked for, so surface it instead of silently recording half the conversation.
        let handle = crate::system_audio::start(&app).map_err(|err| {
            format!("System audio unavailable: {err} Grant Screen Recording to Flow, then try again.")
        })?;
        inner.system_audio = Some(handle);
    }

    emit_status(&app, &inner);
    Ok(status_from(&inner))
}

#[tauri::command]
pub fn meeting_stop_capture(app: AppHandle) -> Result<MeetingTranscript, String> {
    let state = app.state::<MeetingSession>();
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;

    if inner.phase != MeetingPhase::Capturing {
        return Err("No meeting is currently being captured.".into());
    }

    #[cfg(target_os = "macos")]
    if let Some(handle) = inner.system_audio.take() {
        crate::system_audio::stop(handle);
    }

    let transcript = MeetingTranscript {
        id: Uuid::new_v4().to_string(),
        app_name: inner.app_name.clone().unwrap_or_else(|| "Unknown".into()),
        started_at: inner.started_at_wall.clone().unwrap_or_else(now_epoch_secs),
        ended_at: now_epoch_secs(),
        segments: std::mem::take(&mut inner.segments),
    };

    store::add_meeting_transcript(transcript.clone())?;

    inner.phase = MeetingPhase::Idle;
    inner.session_id = None;
    inner.started_at = None;
    inner.started_at_wall = None;
    // app_name is left as-is; the detection loop clears it once the process actually exits.

    emit_status(&app, &inner);
    Ok(transcript)
}

#[tauri::command]
pub async fn meeting_submit_mic_chunk(app: AppHandle, audio_base64: String) -> Result<(), String> {
    {
        let state = app.state::<MeetingSession>();
        let inner = state
            .inner
            .lock()
            .map_err(|_| "Meeting session lock poisoned".to_string())?;
        if inner.phase != MeetingPhase::Capturing {
            return Err("Not currently capturing a meeting.".into());
        }
    }

    let bytes = STANDARD
        .decode(audio_base64.trim())
        .map_err(|e| format!("Invalid audio data: {e}"))?;
    if bytes.is_empty() {
        return Ok(());
    }

    ingest_chunk(&app, "you", bytes).await
}

/// Shared by the mic Tauri command and (once wired up) the system-audio chunker. Silently drops
/// chunks that arrive after the session has already ended instead of erroring — that's a normal
/// race between the frontend's chunk timer and a manual Stop, not a failure.
pub async fn ingest_chunk(
    app: &AppHandle,
    speaker: &str,
    audio_bytes: Vec<u8>,
) -> Result<(), String> {
    let session_id = {
        let state = app.state::<MeetingSession>();
        let inner = state
            .inner
            .lock()
            .map_err(|_| "Meeting session lock poisoned".to_string())?;
        if inner.phase != MeetingPhase::Capturing {
            return Ok(());
        }
        inner.session_id.clone().unwrap_or_default()
    };

    let config = load_config();
    let text_result = if cloud_api::uses_cloud(&config) {
        let b64 = STANDARD.encode(&audio_bytes);
        cloud_api::meeting_transcribe(&config, &b64, speaker, &session_id).await
    } else {
        crate::stt_local::transcribe(&config, audio_bytes).await
    };

    let text = match text_result {
        Ok(t) if !t.trim().is_empty() => t.trim().to_string(),
        Ok(_) => return Ok(()), // silence in this chunk — not an error
        Err(err) => {
            let _ = app.emit("meeting-error", err);
            return Ok(());
        }
    };

    let state = app.state::<MeetingSession>();
    let mut inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;
    if inner.phase != MeetingPhase::Capturing {
        return Ok(());
    }
    let at_ms = inner
        .started_at
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or(0);
    inner.segments.push(TranscriptSegment {
        speaker: speaker.to_string(),
        text: text.clone(),
        at_ms,
    });
    let _ = app.emit(
        "meeting-transcript-partial",
        serde_json::json!({ "speaker": speaker, "text": text, "at_ms": at_ms }),
    );

    Ok(())
}

/// Optional background poll for known call-app processes. It is intentionally not started by
/// default: a process being open is not evidence that a call is active, and false banners make
/// the Hub look broken. Keep this available for a future call-state-aware detector.
#[allow(dead_code)]
pub fn start_detection_loop(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        let running = detect_running_app();

        let state = app.state::<MeetingSession>();
        let mut inner = match state.inner.lock() {
            Ok(g) => g,
            Err(_) => continue,
        };

        match (&running, inner.phase) {
            (Some((_, display)), MeetingPhase::Idle) => {
                inner.app_name = Some(display.clone());
                inner.phase = MeetingPhase::Detected;
                let _ = app.emit(
                    "meeting-app-detected",
                    serde_json::json!({ "app": display, "display_name": display }),
                );
            }
            (None, MeetingPhase::Detected) => {
                inner.app_name = None;
                inner.phase = MeetingPhase::Idle;
                let _ = app.emit("meeting-app-cleared", ());
            }
            (None, MeetingPhase::Capturing) => {
                drop(inner);
                let _ = meeting_stop_capture(app.clone());
                continue;
            }
            _ => {}
        }
    });
}

#[allow(dead_code)]
fn detect_running_app() -> Option<(String, String)> {
    let output = Command::new("ps").args(["-axo", "comm="]).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for (needle, display) in KNOWN_APPS {
        if stdout.lines().any(|line| line.contains(needle)) {
            return Some((needle.to_string(), display.to_string()));
        }
    }
    None
}

/// Screen Recording is OS-level TCC (same mechanism class as Accessibility, checked the same way
/// `check_accessibility_trusted` does in lib.rs) — no extra crate needed just to preflight it.
#[tauri::command]
pub fn check_screen_recording_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGPreflightScreenCaptureAccess() -> bool;
        }
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[derive(Debug, Serialize)]
pub struct ScreenRecordingStatus {
    pub trusted: bool,
}

#[tauri::command]
pub fn get_screen_recording_status() -> ScreenRecordingStatus {
    ScreenRecordingStatus {
        trusted: check_screen_recording_trusted(),
    }
}

#[tauri::command]
pub fn open_screen_recording_settings() -> Result<(), String> {
    Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .spawn()
        .map_err(|e| format!("Could not open Screen Recording settings: {e}"))?;
    Ok(())
}

pub fn latest_question_context(app: &AppHandle) -> Result<MeetingQuestionContext, String> {
    let state = app.state::<MeetingSession>();
    let inner = state
        .inner
        .lock()
        .map_err(|_| "Meeting session lock poisoned".to_string())?;

    if inner.phase != MeetingPhase::Capturing {
        return Err(
            "No live conversation is being captured. Start meeting transcription first.".into(),
        );
    }
    if inner.segments.is_empty() {
        return Err("No live conversation text has been captured yet.".into());
    }

    let question = pick_latest_question(&inner.segments)
        .ok_or_else(|| "No question has been detected in the live conversation yet.".to_string())?;
    let transcript = inner
        .segments
        .iter()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|segment| format!("{}: {}", segment.speaker, segment.text.trim()))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(MeetingQuestionContext {
        question,
        transcript,
    })
}

fn pick_latest_question(segments: &[TranscriptSegment]) -> Option<String> {
    let mut newest_any = None;
    let mut newest_other = None;
    for segment in segments.iter().rev() {
        if let Some(question) = extract_question(&segment.text) {
            if newest_any.is_none() {
                newest_any = Some(question.clone());
            }
            if segment.speaker != "you" {
                newest_other = Some(question);
                break;
            }
        }
    }
    newest_other.or(newest_any)
}

fn extract_question(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    if let Some(idx) = text.rfind('?') {
        let before = &text[..=idx];
        let start = before
            .rfind(|c| matches!(c, '.' | '!' | '\n'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let question = before[start..].trim();
        if !question.is_empty() {
            return Some(question.to_string());
        }
    }

    let lower = text.to_lowercase();
    let looks_like_question = [
        "can you ",
        "could you ",
        "do you ",
        "does ",
        "how ",
        "should ",
        "what ",
        "when ",
        "where ",
        "which ",
        "who ",
        "why ",
        "would you ",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix));

    looks_like_question.then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker: speaker.into(),
            text: text.into(),
            at_ms: 0,
        }
    }

    #[test]
    fn prefers_newest_other_question() {
        let segments = vec![
            seg("other", "What is the deadline?"),
            seg("you", "Can you send the doc?"),
            seg("other", "Should we ship Friday?"),
        ];
        assert_eq!(
            pick_latest_question(&segments).as_deref(),
            Some("Should we ship Friday?")
        );
    }

    #[test]
    fn falls_back_to_newest_user_question() {
        let segments = vec![
            seg("you", "What is the status?"),
            seg("you", "Can you confirm the time?"),
        ];
        assert_eq!(
            pick_latest_question(&segments).as_deref(),
            Some("Can you confirm the time?")
        );
    }
}
