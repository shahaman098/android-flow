# Sibling repo — read before editing shared folders

Sprout is one product with two clients, living in two separate git repos on this Mac.

| | This repo | Sibling |
|---|---|---|
| Path | `Projects/Android Flow` | `Projects/Wispr Flow` |
| Remote | `shahaman098/android-flow` | `shahaman098/wispr-flow` |
| Client | Android — Kotlin/Compose (`android/`, `store/`) | macOS — Tauri 2 + React/TS (`src-tauri/`, `src/`) |
| Trigger | floating bubble overlay | hold `fn`, `fn+1`, `fn+2`, `fn+3` |

Shared history up to `ada3726`; this repo forked at `5ccbddd`. There is no `src-tauri/` or
`src/` here — the macOS app is not part of this tree.

## Folders that exist in both as forked copies — not symlinks

`cloud/`, `constitutions/`, `skills/`, `context/`, `docs/`

Editing one does **not** update the other. Already diverged:

- `cloud/app/llm.py`, `cloud/app/main.py`, `cloud/app/stt.py`
- `skills/README.md`, `skills/grammar-correct/SKILL.md`
- `context/*`, `docs/*` — expected, each describes its own client
- `constitutions/vibe-coding.md` is still byte-identical. Keep it that way.

**`cloud/` is the sharp edge.** Both clients call the *same* deployed Cloud Run service
`flow-api`, but each repo now holds its own diverged source for it. Whoever deploys last
silently overwrites the other client's backend. Before touching `cloud/`, diff it against
the sibling and decide which repo owns the deploy.

## Vibe prompts load context from one repo only

The macOS app's `config.json` → `vibe_project_root` is a single fixed path, and it feeds the
prompt generator. It currently points at **`Projects/Wispr Flow`**, not here — so prompts
generated while you work on Android arrive describing Tauri 2 and macOS Accessibility APIs
instead of Kotlin and Compose.

Repoint it when you switch repos, and check it before trusting a generated prompt.
