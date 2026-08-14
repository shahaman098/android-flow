# Privacy — Sprout

Last updated: 2026-08-14

Public URL (after GitHub Pages is enabled on this repo):
https://shahaman098.github.io/android-flow/privacy.html

Contact: sahkris0844@gmail.com

## What Sprout is

Sprout is a dictation and Vibe Coding helper. A floating bubble records your voice and inserts text into the app you are using. You configure your own `flow-api` URL and API key. The Play Store listing does not include a hosted Flow backend.

## Data we process

| Data | Why | Where |
|---|---|---|
| Microphone audio | Speech-to-text for dictation | Sent to the HTTPS `flow-api` endpoint **you** configure, then discarded by the Android client |
| Focused text field contents | Vibe prompt and grammar correction | Sent to that same `flow-api` for LLM processing |
| API URL and API key | Authenticate to your backend | Stored only on-device (Android DataStore) |

The Android app does not operate a default cloud. If you run `flow-api` yourself (for example Cloud Run with Groq STT and DeepSeek LLM), **you** are the operator of that backend and those providers process audio/text under **your** accounts.

## Permissions

- **Microphone** — record dictation audio after an in-app disclosure
- **Display over other apps** — floating control bubble
- **Accessibility** — read the focused editable field and insert results. Not used to scrape unrelated screen content
- **Internet** — call your `flow-api` over HTTPS in release builds
- **Notifications** — foreground service status while the bubble runs

## What we do not do

- No account system
- No analytics SDK
- No advertising identifiers
- No sale of audio or transcripts
- No reading of non-editable or non-focused UI beyond finding the active field

## Retention

The Android client does not keep recordings. Audio and field text live only long enough to send the request. Retention on `flow-api` and third-party STT/LLM providers is controlled by whoever operates that API.

## Contact

sahkris0844@gmail.com
