# Skill: speech-to-text

Converts speech input to text via the configured speech provider.

- **local / hybrid**: Mac STT (`stt_local.rs` — whisper.cpp, Groq, or GCP)
- **cloud**: Cloud Run `/v1/transcribe` (Groq by default)

## Trigger

Recording session started by holding **fn**.

## Implementation

- Frontend captures audio in ~50s windows (`src/lib/audio.ts`) so providers with a
  60s sync cap (GCP Speech) never see one giant blob
- Backend router: `src-tauri/src/stt_local.rs`
- Dictionary terms are passed as an STT prompt (Groq / local Whisper) or phrase-set
  (GCP)
- After STT, hold `fn` runs light cleanup + snippet expansion unless cleanup is off
- `[FILL: custom vocabulary]`

## Output

Cleaned transcript by default (fillers/take-backs removed). Raw STT is kept when
light cleanup is disabled or the cleanup pass is rejected as unsafe.
