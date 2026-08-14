# Project context — Android Flow

Use this file (and siblings under `context/`) when generating vibe prompts from the
floating bubble **Vibe prompt** action.

## Product

- Name: Sprout (Android companion; package `com.efi.androidflow`)
- Mascot: Spout (whale with speech bubble — launcher icon, `store/icon-512.png`,
  `drawable/ic_spout.png`)
- Kind: Android companion for dictation + Vibe Coding prompts (floating bubble +
  Accessibility insert)
- Stack: Kotlin / Jetpack Compose shell + shared Cloud Run `flow-api`

## Architecture

- Android captures mic audio and focused-field text only.
- Processing runs on Cloud Run service `flow-api`
  (`POST /v1/transcribe`, `POST /v1/process`) authenticated with `FLOW_API_KEY`.
- Default cloud STT: Groq `whisper-large-v3-turbo`.
- Default cloud LLM: DeepSeek `deepseek-v4-flash`.
- A floating bubble (overlay) shows idle / recording / processing / error state.
- Current cloud state: billing may be disabled for `project-ced3b331-e814-4d72-8bc` —
  see `docs/cloud-hosting.md`. Do not silently re-enable billing.

## Controls (do not change intended meaning)

| Control | Behavior |
|---|---|
| Hold bubble | Speech → STT → light cleanup → insert at focus (Accessibility required) |
| Bubble → Vibe prompt | Read focused text → merge `context/` → perfect Vibe Coding prompt → replace field |
| Bubble → Fix grammar | Read focused text → grammar/spelling cleanup → replace field |

Languages: English, Hindi (हिन्दी), Nepali (नेपाली) — Hub language sets both UI locale and STT/LLM output.

Meeting / live-conversation answer (Mac `fn+3`) is deferred on Android.

## Active feature priorities

- Prompt-generation quality is the top priority (faithful, well-formed prompts).
- Vibe prompt runs draft → template/proper-noun check → repair only when needed.
- Hold-bubble dictation runs light cleanup when `correct_english` is on.
- Fix grammar remains the explicit style-aware cleanup of the current field.

## Forbidden changes / do-nots

- Do not silently recreate cloud hosting, switch LLM providers, or re-enable billing.
- Do not change what hold-bubble / vibe / grammar do at a high level without an explicit request.
- Do not drop any of the ten canonical template sections in `constitutions/vibe-coding.md`.
- Do not add fallback paths (mechanical LLM templates, clipboard insert, silent provider switching). No further fallback development.
- Do not add macOS-only client code (`src-tauri/`, Tauri, fn hotkeys, side dock, local Whisper, ScreenCaptureKit). That lives in wispr-flow.

## Notes

- UI is Kotlin + Jetpack Compose for native overlay + Accessibility quality.
- `[FILL: architecture decisions beyond the above]`
