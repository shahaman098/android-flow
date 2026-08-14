# Project context — Vibe Coding / Flow

Use this file (and siblings under `context/`) when creating prompts with **fn+1**.

## Product

- Name: Flow (Wispr Flow workspace)
- Kind: personal Mac dictation + Vibe Coding prompt helper, works in any text field system-wide
  (macOS Accessibility APIs for capture + paste)
- Stack: Tauri 2 (Rust backend) + React/TypeScript frontend on the Mac. Three processing
  modes: local, hybrid, and cloud.

## Architecture

- Local: Mac captures mic audio, transcribes with whisper.cpp (or Groq/GCP), then calls the
  DeepSeek/xAI LLM API for cleanup/prompting. Fresh installs default here unless Cloud Run
  credentials already exist.
- Hybrid: Mac STT + Cloud Run LLM. Audio stays on the Mac; cleanup, vibe, grammar, and spoken
  edits go to `flow-api` `/v1/process` with the transcript (no audio). Meetings stay local STT.
- Current cloud state: decommissioned. Billing is disabled/unlinked for
  `project-ced3b331-e814-4d72-8bc`.
- Cloud: Mac captures mic audio and keypresses only, then sends processing to
  Cloud Run service `flow-api` (`POST /v1/transcribe`, `POST /v1/process`) authenticated with
  `FLOW_API_KEY`.
- Default cloud STT: Groq `whisper-large-v3-turbo`.
- Default cloud LLM: DeepSeek `deepseek-v4-flash`.
- Historical expensive LLM: Qwen served by Ollama on dedicated GCE VM `flow-llm`, fronted by an
  auth proxy. Do not recreate this for "maximum cheap" cloud mode.
- A side dock window (always-on-top, draggable) shows idle/recording/processing/error state.

## Hotkeys (do not change intended meaning)

| Shortcut | Behavior |
|---|---|
| hold `fn` | Speech → STT (chunked ~50s) → light cleanup + snippet expand → paste. If text is selected, a short spoken instruction edits that range instead |
| `fn+1` | Select-all current text → merge `context/` → perfect Vibe Coding prompt → pasted back |
| `fn+2` | Select-all current text → grammar/spelling cleanup → pasted back |
| `fn+3` | Answer the latest question detected in the live conversation transcript → pasted at cursor |

## Active feature priorities

- Prompt-generation quality is the top priority right now (target: consistently well-formed,
  faithful-to-source prompts) — latency is a secondary concern as long as the dock clearly shows
  "still working".
- fn+1 runs a draft pass followed by an automated template/proper-noun
  check, with a repair pass triggered only when the check fails — this keeps the common case fast
  while catching malformed output.
- Hold fn runs light cleanup (fillers, punctuation, take-backs) when `correct_english` is on.
  fn+2 remains the explicit style-aware cleanup of the current field.

## Forbidden changes / do-nots

- Do not silently recreate cloud hosting, switch LLM providers, or re-enable billing. The previous
  MyGCP open-weight VM stack is documented in `docs/cloud-hosting.md`; it is not the cheap
  default.
- Do not change what fn / fn+1 / fn+2 / fn+3 do at a high level without an explicit request.
- Do not drop any of the ten canonical template sections defined in
  `constitutions/vibe-coding.md` when generating a prompt.

## Notes

- [FILL: architecture decisions beyond the above]
