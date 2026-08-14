# Skill: speech-to-text

Converts speech input to text via the configured `flow-api` speech provider.

- **Android:** floating bubble records PCM/WAV and sends it to Cloud Run `/v1/process` (`mode=dictate`)
- **cloud STT:** Groq `whisper-large-v3-turbo` by default (`STT_PROVIDER=gcp_speech` is an explicit deploy choice)

## Trigger

Recording session started by **holding the floating bubble**.

## Implementation

- Mic capture: `android/.../audio/MicRecorder.kt` (16 kHz mono PCM16, wrapped as WAV)
- Overlay: `android/.../service/BubbleOverlayService.kt` hold-to-talk
- Insert: `android/.../service/FlowAccessibilityService.kt` into the focused editable field
- Dictionary terms are passed through to `flow-api` (Groq prompt or GCP phrase-set)
- After STT, hold-bubble runs light cleanup unless Hub **Light cleanup** is off

## Output

Cleaned transcript by default (fillers/take-backs removed). Raw STT is kept when
light cleanup is disabled or the cleanup pass is rejected as unsafe.
