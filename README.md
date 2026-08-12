# Flow — Vibe Coding voice prompts

Personal Mac helper for **speech → grammar → Vibe Coding prompts**.

| Shortcut | Pipeline |
|---|---|
| **hold `fn`** | Speech → GCP STT → grammar cleanup → paste |
| **hold `fn` + `1`** | Speech → GCP STT → grammar cleanup → **perfect Vibe Coding prompt** → paste |
| **`fn` + `2`** | Select-all current prompt → load `context/` → **refined Vibe Coding prompt** → paste |

`fn` (the Globe key) is the primary hotkey and is always hold-to-talk. **Control+1** and
**Control+2** remain registered as fallbacks for the prompt and refine pipelines — useful
when the `fn` event tap cannot start. Control+1 honours the *Hands-free* setting (tap to
start, tap to stop); `fn` does not.

Do not change the intended meaning of these shortcuts.

## Folder hierarchy

```text
Wispr Flow/
├── constitutions/
│   └── vibe-coding.md          # Rules for prompt generation
├── context/
│   ├── README.md
│   └── project.md              # Project facts for Control+2
├── skills/
│   ├── README.md
│   ├── speech-to-text/SKILL.md
│   ├── grammar-correct/SKILL.md
│   ├── vibe-prompt/SKILL.md    # Control+1
│   └── refine-prompt/SKILL.md  # Control+2
├── src/                        # React Hub + bubble UI
└── src-tauri/src/
    ├── stt_gcp.rs              # Speech-to-text (GCP)
    ├── dictate.rs              # Grammar + vibe / refine prompts
    ├── vibe_context.rs         # Loads context/, skills/, constitutions/
    └── focus.rs                # Select-all / paste into target app
```

`[FILL: add more context/*.md as the project grows]`

## Providers (MyGCP)

| Stage | Where |
|---|---|
| Mic + paste | Mac Flow.app |
| Speech-to-text | Cloud Run `flow-api` → Speech V2 (`europe-west2`) |
| Grammar / Vibe prompts | Cloud Run `flow-api` → **Qwen2.5-3B** (Ollama on `flow-llm` VM) |

`cloud/deploy.sh` writes the live API URL and key into
`~/Library/Application Support/voice-flow/.env`, and Flow copies them into `config.json`
on first run. Both are editable afterwards in **Hub → Settings**, which is authoritative —
the `.env` values only fill in fields that are still blank.

```bash
gcloud config set account sahkris0844@gmail.com
gcloud config set project project-ced3b331-e814-4d72-8bc
bash cloud/deploy.sh   # deploy / refresh Cloud Run + local FLOW_API_* keys
```

Mac keeps only mic/paste; processing runs on MyGCP (`PROCESSING_MODE=cloud`).

## Run

```bash
pnpm install
pnpm start
```

`pnpm start` builds, code-signs, and installs to `/Applications/Flow.app`.

Requires Xcode or the Command Line Tools (`xcode-select --install`) — the meeting-capture
path links against Swift via ScreenCaptureKit. `src-tauri/build.rs` locates the Swift
compatibility archives itself, so a Command Line Tools-only machine links correctly; set
`DEVELOPER_DIR` if your toolchain lives somewhere non-standard.

Development and checks:

```bash
pnpm dev
```

```bash
pnpm check
```

`pnpm check` typechecks the frontend and runs the Rust unit tests.

### Accessibility (required — `fn` does nothing without it)

The `fn` hotkey uses a CGEventTap, which macOS refuses to create unless the app is
trusted for Accessibility. Grant it **once**, against `/Applications/Flow.app`:

> System Settings → Privacy & Security → Accessibility → **+** → `/Applications/Flow.app`

Delete any older `Flow` rows there first; stale entries point at a different
signature and silently block the new one.

The build signs with a real Apple Development identity so the designated
requirement is `identifier "com.efi.voiceflow" and anchor apple generic and …`
— no cdhash — which is why the approval now survives every rebuild. An ad-hoc
(linker-signed) bundle has no designated requirement at all, so macOS could not
record the grant and `AXIsProcessTrusted()` stayed false forever.

Check it took: `/tmp/flow-section.log` should read `AXIsProcessTrusted=true`
followed by `CGEventTapCreate OK`.

## Checklist

- [x] Speech-to-text (`stt_gcp.rs` + `skills/speech-to-text`)
- [x] Grammar auto-correction (`polish_text` + `skills/grammar-correct`)
- [x] Control+1 Vibe prompt (`vibe` mode + `skills/vibe-prompt`)
- [x] Control+2 context refine (`vibe_refine` + `skills/refine-prompt`)
- [x] Folder structure `context/`, `skills/`, `constitutions/`
- [x] Inline comments on each component
