# Play Console — Sprout

Package: `com.efi.androidflow`  
Version: `1.0.0` (versionCode `1`)  
AAB: `android/app/build/outputs/bundle/release/app-release.aab`  
High-res icon: `store/icon-512.png` (512×512)  
Feature graphic: `store/feature-graphic.png` (1024×500)

This app is **not** a hosted dictation service. Users paste their own `FLOW_API_URL` and `FLOW_API_KEY`. Do not re-enable GCP billing as part of this listing.

## Before upload

1. Back up `android/app/flow-release.jks` and `android/keystore.properties` somewhere safe (not git). Losing them means you cannot update this listing.
2. Privacy URL (GitHub Pages on **android-flow**, `/docs`):
   `https://shahaman098.github.io/android-flow/privacy.html`
3. Privacy contact is `sahkris0844@gmail.com` (same as the GCP account in `docs/cloud-hosting.md`).
4. Capture phone screenshots (see sizes below).
5. `cd android && export JAVA_HOME=$(/usr/libexec/java_home -v 21) && ./gradlew :app:bundleRelease`

## Store listing copy

**App name:** Sprout

**Short description (≤80 chars):**
Dictate into any app. Hold the bubble for speech, vibe prompts, and grammar.

**Full description:**

Sprout is a floating bubble for dictation and Vibe Coding prompts.

Hold the bubble to speak into the focused text field. Tap the bubble for a Vibe Coding prompt or grammar cleanup. English, Hindi (हिन्दी), and Nepali (नेपाली) are supported in the Hub.

This app is a client. You bring your own flow-api:

1. Run or host flow-api (see the project README).
2. Paste FLOW_API_URL and FLOW_API_KEY in Hub → Save → Test API.
3. Allow Microphone, Display over other apps, and Accessibility.
4. Launch the bubble, open Notes, focus a field, hold to dictate.

Audio and focused-field text are sent only to the HTTPS endpoint you configure. The app does not include a default cloud backend.

Privacy: https://shahaman098.github.io/android-flow/privacy.html

**Category:** Productivity  
**Tags:** dictation, speech-to-text, writing, accessibility helper

## Data safety (copy-paste)

| Question | Answer |
|---|---|
| Does your app collect or share user data? | Yes — collected, not shared with other companies by the Android client |
| Location | No |
| Personal info (name, email, ids) | No |
| Financial | No |
| Health | No |
| Messages | No |
| Photos / video | No |
| Audio files | Yes — microphone audio, collected, ephemeral on device, sent to user-configured flow-api |
| Files and docs | No |
| Calendar | No |
| Contacts | No |
| App activity | No |
| Web browsing | No |
| App info and performance | No |
| Device or other IDs | No |
| Is data encrypted in transit? | Yes (HTTPS in release) |
| Can users request deletion? | Data is not retained in the app. Deletion on flow-api is the operator’s responsibility |
| Data sold | No |
| Data used for ads | No |
| Data used for fraud prevention / security / personalization | No |

Collected data types to declare: **Microphone / voice recordings** (App functionality), **User-generated content / text** (focused field contents for vibe/grammar).

## Permissions declarations

### Accessibility

- **Is this an accessibility app?** Declare use of the Accessibility API.
- **Why:** Insert and replace text in the currently focused editable field after dictation, vibe prompt, or grammar cleanup. Accessibility is required; there is no clipboard fallback.
- **Does not:** read other apps’ screens beyond the focused editable field, click UI, or scrape content.

### Display over other apps

Floating dictation bubble (always-on-top control), equivalent to a system-wide helper.

### Foreground service `specialUse`

Keep the bubble alive while the user dictates into other apps. Subtype already in the manifest: “Floating dictation bubble for inserting text into other apps”.

### Microphone

In-app disclosure appears before the system permission. Audio is sent to the user-configured flow-api for transcription only.

## Content rating

Questionnaire: **Tools / Productivity / Reference**. No user-generated social features, no ads, no in-app purchases in this build, no violence, no dating.

## Reviewer notes (paste into Console)

Sprout requires a user-provided flow-api.

Test steps:
1. Open Hub.
2. Paste FLOW_API_URL and FLOW_API_KEY (HTTPS). Save. Test API.
3. Enable Microphone (confirm the disclosure), Display over other apps, and Accessibility → Sprout.
4. Launch floating bubble.
5. Open any notes app, focus a field, hold the bubble to dictate. Tap the bubble for Vibe prompt / Fix grammar.

Without API credentials the Hub still opens; dictation will error until keys are set. That is intended.

## Screenshots (you capture on device)

| Asset | Size |
|---|---|
| Phone screenshots | at least 2, 16:9 or 9:16, min 320px on short side |
| Suggested shots | Hub onboarding; Hub with language chips; bubble over Notes while listening |
| Feature graphic | `store/feature-graphic.png` 1024×500 |
| Icon | `store/icon-512.png` 512×512 |

## GitHub Pages

Repo Settings → Pages → Source: Deploy from branch `main`, folder `/docs`.  
Then `https://shahaman098.github.io/android-flow/privacy.html` must 200 before you submit. The `pages.yml` workflow publishes `docs/` from `main`.
