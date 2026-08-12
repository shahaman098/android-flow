# Project context — Vibe Coding / Flow

Use this file (and siblings under `context/`) when refining prompts with **Control+2**.

## Product

- Name: Flow (Wispr Flow workspace)
- Kind: personal Mac dictation + Vibe Coding prompt helper, works in any text field system-wide
  (macOS Accessibility APIs for capture + paste)
- Stack: Tauri 2 (Rust backend) + React/TypeScript frontend on the Mac; all STT/LLM processing
  runs on MyGCP (`project-ced3b331-e814-4d72-8bc`, `europe-west2`) behind a Cloud Run API

## Architecture

- Mac app captures mic audio and keypresses only — it does not run STT or the LLM locally when
  `processing_mode = cloud` (the default).
- Cloud Run service `flow-api` exposes `POST /v1/transcribe` and `POST /v1/process`, authenticated
  with a bearer `FLOW_API_KEY` (Secret Manager: `flow-api-key`).
- STT: Google Cloud Speech-to-Text V2, model `latest_long`, `en-GB`.
- LLM: Qwen2.5-14B-Instruct (open-weight) served by Ollama on a dedicated GCE VM (`flow-llm`,
  `n2-standard-8`, CPU-only — project GPU quota is currently 0), fronted by an auth proxy.
- A side dock window (always-on-top, draggable) shows idle/recording/processing/error state.

## Hotkeys (do not change intended meaning)

| Shortcut | Behavior |
|---|---|
| Control+1 | Speech → GCP STT → grammar cleanup → perfect Vibe Coding prompt → pasted at cursor |
| Control+2 | Select-all the prompt currently on screen → merge `context/` → refined Vibe Coding prompt → pasted back |

## Active feature priorities

- Prompt-generation quality is the top priority right now (target: consistently well-formed,
  faithful-to-source prompts) — latency is a secondary concern as long as the dock clearly shows
  "still working".
- Both Control+1 and Control+2 run a draft pass followed by an automated template/proper-noun
  check, with a repair pass triggered only when the check fails — this keeps the common case fast
  while catching malformed output.

## Forbidden changes / do-nots

- Do not silently fall back to a different LLM provider or reintroduce API-key-based cloud LLMs
  (OpenAI/DeepSeek) as a default — this project intentionally runs an open-weight model on MyGCP.
- Do not change what Control+1 / Control+2 do at a high level without an explicit request.
- Do not drop any of the ten canonical template sections defined in
  `constitutions/vibe-coding.md` when generating or refining a prompt.

## Notes

- [FILL: architecture decisions beyond the above]
