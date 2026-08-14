# Flow — Vibe Coding voice prompts

Personal Mac helper for **speech → grammar → Vibe Coding prompts**.

| Shortcut | Pipeline |
|---|---|
| **hold `fn`** | Speech → STT → light cleanup → snippet expand → paste |
| **`fn` + `1`** | Select-all current text → load `context/` → **perfect Vibe Coding prompt** → paste |
| **`fn` + `2`** | Select-all current text → grammar/spelling cleanup → paste |
| **`fn` + `3`** | Answer the latest question detected in the live conversation transcript → paste |

`fn` (the Globe key) is the primary control. Hold `fn` to dictate with light
cleanup (disable in Settings for raw STT). If text is already selected, a short
spoken command edits that range. `fn+1` turns the current text into a Vibe
Coding prompt, `fn+2` corrects the current field, and `fn+3` answers the latest
detected live-conversation question.

Do not change the intended meaning of these shortcuts.

## Folder hierarchy

```text
Wispr Flow/
├── constitutions/
│   └── vibe-coding.md          # Rules for prompt generation
├── context/
│   ├── README.md
│   └── project.md              # Project facts for fn+1 prompt generation
├── skills/
│   ├── README.md
│   ├── speech-to-text/SKILL.md
│   ├── grammar-correct/SKILL.md
│   └── vibe-prompt/SKILL.md    # fn+1
├── src/                        # React Hub + bubble UI
└── src-tauri/src/
    ├── stt_groq.rs             # Speech-to-text (Groq)
    ├── stt_gcp.rs              # Speech-to-text fallback (GCP)
    ├── dictate.rs              # Light-cleaned dictation + prompt/correction/answer transforms
    ├── vibe_context.rs         # Loads context/, skills/, constitutions/
    └── focus.rs                # Select-all / paste into target app
```

`[FILL: add more context/*.md as the project grows]`

## Providers

| Stage | Where |
|---|---|
| Mic + paste | Always local in Mac Flow.app |
| Local | Mac STT (whisper.cpp or Groq) + Mac LLM (DeepSeek / xAI) |
| Hybrid | Mac STT + Cloud Run LLM (`flow-api` `/v1/process` with transcript, no audio) |
| Cloud | Mac records/pastes only → Cloud Run STT + LLM |

Fresh installs default to local unless `FLOW_API_URL` and `FLOW_API_KEY` are already
configured. Cloud deploys set `PROCESSING_MODE=cloud` automatically. Switch to hybrid
in **Hub → Settings** to keep speech on the Mac while using Cloud Run for cleanup and
prompts.

### Local mode

```bash
export DEEPSEEK_API_KEY=...
bash scripts/configure-local.sh
pnpm start
```

The Mac records, transcribes, and pastes locally, then calls DeepSeek for cleanup/prompting. No
Cloud Run, GCP billing, or STT API key is required for this mode.

Prerequisites:

```bash
brew install whisper-cpp ffmpeg
mkdir -p "$HOME/Library/Application Support/voice-flow/models"
curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin \
  -o "$HOME/Library/Application Support/voice-flow/models/ggml-small.en.bin"
```

Optional low-CPU Groq STT instead of local Whisper:

```bash
export STT_PROVIDER=groq_whisper
export GROQ_API_KEY=...
bash scripts/configure-local.sh
```

Optional xAI/Grok LLM instead of DeepSeek:

```bash
export LLM_PROVIDER=xai
export XAI_API_KEY=...
bash scripts/configure-local.sh
```

### Hybrid mode

Set `PROCESSING_MODE=hybrid` (or pick **Hybrid** in Hub Settings) when Cloud Run is
deployed and you still want Mac speech. Dictation transcribes locally, then sends the
transcript to Cloud Run for light cleanup, vibe prompts, grammar, and spoken edits.
Meetings stay on local STT.

```bash
# After cloud/deploy.sh has written FLOW_API_*
# Hub → Settings → Processing mode → Hybrid
```

### Cloud mode

`cloud/deploy.sh` deploys the same processing pipeline behind Cloud Run, then writes the live API
URL and key into
`~/Library/Application Support/voice-flow/.env`, and Flow copies them into `config.json`
on first run. Both are editable afterwards in **Hub → Settings**, which is authoritative —
the `.env` values only fill in fields that are still blank.

The default deployment scales Cloud Run to zero and does not create the old `flow-llm`
Compute Engine VM. Add `GROQ_API_KEY` and `DEEPSEEK_API_KEY` to the app `.env` before
deploying; use `STT_PROVIDER=gcp_speech bash cloud/deploy.sh` only if you deliberately
want the more expensive GCP Speech fallback.

```bash
gcloud config set account sahkris0844@gmail.com
gcloud config set project project-ced3b331-e814-4d72-8bc
bash cloud/deploy.sh   # deploy / refresh Cloud Run + local FLOW_API_* keys
```

Mac keeps only mic/paste; STT and prompt processing run through Cloud Run
(`PROCESSING_MODE=cloud`).

Optional xAI/Grok Cloud Run deploy:

```bash
export GROQ_API_KEY=...
export LLM_PROVIDER=xai
export XAI_API_KEY=...
bash cloud/deploy.sh
```

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
- [x] fn+1 Vibe prompt from current text (`vibe_text` mode + `skills/vibe-prompt`)
- [x] fn+2 text correction (`correct_text` mode + `skills/grammar-correct`)
- [x] fn+3 live conversation question answering (`meeting_answer` mode)
- [x] Folder structure `context/`, `skills/`, `constitutions/`
- [x] Inline comments on each component
