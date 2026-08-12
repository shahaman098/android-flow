# Skill: meeting-transcript

Consent-gated local meeting transcription: captures the user's mic and (once macOS Screen
Recording permission is granted) the remote participants' system audio via ScreenCaptureKit,
transcribes both in periodic chunks via Google Cloud Speech-to-Text V2, and persists only the
resulting text transcript (never raw audio) to the local store.

## Trigger

A supported call app (Zoom, Microsoft Teams, Slack, Discord, WhatsApp, FaceTime) is detected
running on the Mac (`src-tauri/src/meeting.rs::detect_running_app`, polled every 5s), AND the
user explicitly starts transcription from the Hub "Meetings" banner AND confirms they have
notified the other participants (`meeting_confirm_notified` → `meeting_start_capture`).
Detection alone never starts capture. Google Meet (browser tab) is out of scope — it isn't a
process, and detecting it would need a browser extension.

## Implementation

- Detection: `src-tauri/src/meeting.rs::detect_running_app` (macOS-only process-name polling).
- Consent state machine: `src-tauri/src/meeting.rs::MeetingSession` / `MeetingPhase`
  (`Idle → Detected → Notified → Capturing`). `meeting_start_capture` is the enforcement point —
  it refuses to run unless the session is in `Notified`.
- Mic capture: `src/lib/meetingAudio.ts` (`MediaRecorder`, stopped and restarted every 20s on the
  same `MediaStream` so each chunk is an independently valid file) driven by
  `src/lib/useMeetingCapture.ts`, which starts/stops purely in reaction to the backend's
  `meeting-phase-changed` event.
- System audio: `src-tauri/src/system_audio.rs` (ScreenCaptureKit, 48kHz mono, macOS 13+ —
  requires one-time Screen Recording permission; audio only, no video/frames are ever captured
  or stored).
- Transcription: cloud mode → `POST /v1/meeting/transcribe`
  (`cloud/app/main.py::meeting_transcribe`, reuses `cloud/app/stt.py::transcribe`); local mode →
  `src-tauri/src/stt_gcp.rs::transcribe` directly. No LLM/grammar-polish step — raw STT text is
  persisted as-is.
- Persistence: `src-tauri/src/store.rs::MeetingTranscript` / `TranscriptSegment`, viewable and
  deletable in the Hub "Meetings" tab.

## Rules

- Capture never starts without a backend-enforced "notified" confirmation
  (`meeting_confirm_notified` before `meeting_start_capture`) — not just a frontend checkbox.
  `meeting_submit_mic_chunk` independently re-checks the phase too, so a stray call can't inject
  a segment outside a real session.
- No raw audio is ever written to disk — mic and system-audio bytes stay in memory from capture
  through the STT request and are dropped immediately after each ~20s chunk.
- Capture stops on manual stop or when the detected call-app process exits — there is no
  "keep listening" state.
- Speaker label is `"you"` (mic) or `"other"` (system audio) — no voiceprint, diarization, or
  calendar-based named-speaker matching in v1.
- No video or screen content is ever captured — only the system **audio** stream.

## Output

`MeetingTranscript { id, app_name, started_at, ended_at, segments: [{ speaker, text, at_ms }] }`
persisted to the local store, viewable/deletable in the Hub "Meetings" tab.
