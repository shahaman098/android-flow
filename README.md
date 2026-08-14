# Android Flow — Vibe Coding voice prompts

Android companion for **speech → cleanup → Vibe Coding prompts**.
Floating bubble + Accessibility insert, powered by shared Cloud Run `flow-api`.

GitHub: [shahaman098/android-flow](https://github.com/shahaman098/android-flow)  
Mac Flow stays on [shahaman098/wispr-flow](https://github.com/shahaman098/wispr-flow).

| Control | Pipeline |
|---|---|
| **Hold bubble** | Speech → STT → light cleanup → insert into focused field |
| **Tap bubble → Vibe prompt** | Read focused text → load `context/` + constitution → perfect Vibe Coding prompt → replace |
| **Tap bubble → Fix grammar** | Read focused text → grammar/spelling cleanup → replace |

**Languages:** English · हिन्दी · नेपाली — pick in Hub (UI + speech + LLM output).

## Folder hierarchy

```text
Android Flow/
├── android/                    # Kotlin + Jetpack Compose app
│   └── app/
├── cloud/                      # Shared flow-api (Cloud Run)
├── constitutions/
│   └── vibe-coding.md
├── context/
│   └── project.md
├── skills/
│   ├── speech-to-text/
│   ├── grammar-correct/
│   └── vibe-prompt/
├── docs/cloud-hosting.md
└── PRIVACY.md
```

## Run on your phone

### Option A — Android Studio

1. Open the `android/` folder in Android Studio.
2. Connect a phone with USB debugging (or wireless debugging).
3. Run **app** (debug).

### Option B — CLI

Requires **JDK 17 or 21** (Homebrew JDK 26 breaks the Android Gradle Plugin).

```bash
export JAVA_HOME=$(/usr/libexec/java_home -v 21)
export PATH="$JAVA_HOME/bin:$HOME/Library/Android/sdk/platform-tools:$PATH"
cd android
./gradlew :app:installDebug
```

APK path after build:

`android/app/build/outputs/apk/debug/app-debug.apk`

### First-launch checklist (in Hub)

1. Allow **Microphone**
2. Allow **Display over other apps** (floating bubble)
3. Enable **Android Flow** under **Accessibility**
4. Paste `FLOW_API_URL` + `FLOW_API_KEY` → **Save** → **Test API**
5. Tap **Launch floating bubble**

Then open Notes/Messages, focus a field, **hold** the teal bubble to dictate.

## Cloud API

The Android client talks only to `flow-api`:

- `POST /v1/process` with `mode=dictate | vibe_text | correct_text`
- Auth: `Authorization: Bearer FLOW_API_KEY`

### Deploy Cloud Run (billing must be enabled)

See [docs/cloud-hosting.md](docs/cloud-hosting.md). Billing on the GCP project is currently
disabled — re-link billing before deploy, or use local API for device testing.

```bash
export GROQ_API_KEY=...
export DEEPSEEK_API_KEY=...
bash cloud/deploy.sh
```

Enter the printed `FLOW_API_URL` / `FLOW_API_KEY` in the Hub.

### Local API for phone testing (no Cloud Run)

On your Mac (same Wi‑Fi as the phone):

```bash
cd cloud
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
export GROQ_API_KEY=...
export DEEPSEEK_API_KEY=...
export FLOW_API_KEY=dev-local-key
uvicorn app.main:app --host 0.0.0.0 --port 8080
```

In Hub settings set:

```text
FLOW_API_URL=http://YOUR_MAC_LAN_IP:8080
FLOW_API_KEY=dev-local-key
```

Debug builds allow cleartext HTTP for this LAN path.

## Play Store / marketing prep

- Application id: `com.efi.androidflow`
- Version: `1.0.0` (versionCode 1)
- Privacy: [PRIVACY.md](PRIVACY.md) and hosted page `https://shahaman098.github.io/android-flow/privacy.html`
- Console copy-paste: [docs/play-store.md](docs/play-store.md)
- Listing assets: `store/icon-512.png`, `store/feature-graphic.png`
- Release bundle (JDK 21, after `android/keystore.properties` exists):

```bash
export JAVA_HOME=$(/usr/libexec/java_home -v 21)
cd android
./gradlew :app:bundleRelease
```

AAB: `android/app/build/outputs/bundle/release/app-release.aab`

**Back up** `android/app/flow-release.jks` and `android/keystore.properties`. They are gitignored. Losing them blocks Play updates.

Listing checklist:

- [ ] GitHub Pages on `android-flow`: `.github/workflows/pages.yml` deploys `/docs` after push to `main`
- [x] Privacy contact: sahkris0844@gmail.com
- [ ] Phone screenshots uploaded (Hub + bubble over Notes)
- [ ] Feature graphic and 512 icon from `store/`
- [ ] Data safety + Accessibility declarations from docs/play-store.md
- [ ] Content rating questionnaire

## Stack

- **UI language:** Kotlin + Jetpack Compose (Material 3) — native overlays, Accessibility, and Play performance
- **Backend:** FastAPI `cloud/` on Cloud Run
- **Prompt assets:** `constitutions/`, `context/`, `skills/` (bundled into the APK as assets)
