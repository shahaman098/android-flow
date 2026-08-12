# Skill: speech-to-text

Converts speech input to text via Google Cloud Speech-to-Text V2 (MyGCP).

## Trigger

Recording session started by **Control+1** (hold or hands-free tap).

## Implementation

- Frontend captures audio (`src/lib/audio.ts`)
- Backend: `src-tauri/src/stt_gcp.rs`
- Default: `europe-west2` + model `latest_long` + language `en-GB`
  (`latest_short` + `en-US` is rejected in europe-west2)
- `[FILL: custom vocabulary]`

## Output

Raw transcript string (may contain filler words).
