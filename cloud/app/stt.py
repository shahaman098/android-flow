"""Google Cloud Speech-to-Text V2 — runs with Cloud Run ADC / runtime SA."""

from __future__ import annotations

import base64

import google.auth
import google.auth.transport.requests
import httpx

from .settings import settings


def _access_token() -> str:
    credentials, _ = google.auth.default(
        scopes=["https://www.googleapis.com/auth/cloud-platform"]
    )
    credentials.refresh(google.auth.transport.requests.Request())
    if not credentials.token:
        raise RuntimeError("Failed to obtain GCP access token for Speech-to-Text.")
    return credentials.token


def language_bcp47(language: str | None) -> str:
    lang = (language or settings.default_language).strip()
    if lang in ("", "auto", "en"):
        return "en-GB"
    if lang.lower() in ("en-us", "en_us"):
        return "en-US"
    if lang.lower() in ("en-gb", "en_gb"):
        return "en-GB"
    if "-" in lang:
        return lang
    return f"{lang}-GB"


def _adaptation(dictionary: list[str] | None) -> dict | None:
    """Inline phrase-set boost so dictionary/proper-noun terms are recognized correctly
    instead of relying on grammar-correction to guess them after the fact."""
    words = [w.strip() for w in (dictionary or []) if w.strip()]
    if not words:
        return None
    phrases = [{"value": w, "boost": 20} for w in words[:500]]
    return {"phraseSets": [{"inlinePhraseSet": {"phrases": phrases}}]}


async def transcribe(
    audio_bytes: bytes,
    language: str | None = None,
    dictionary: list[str] | None = None,
) -> str:
    if len(audio_bytes) < 1500:
        raise ValueError(f"Audio too short ({len(audio_bytes)} bytes).")

    project = settings.gcp_project_id
    location = settings.gcp_location
    model = settings.stt_model
    lang = language_bcp47(language)
    token = _access_token()
    url = (
        f"https://{location}-speech.googleapis.com/v2/projects/{project}"
        f"/locations/{location}/recognizers/_:recognize"
    )
    config: dict = {
        "autoDecodingConfig": {},
        "languageCodes": [lang],
        "model": model,
    }
    adaptation = _adaptation(dictionary)
    if adaptation:
        config["adaptation"] = adaptation
    payload = {
        "config": config,
        "content": base64.b64encode(audio_bytes).decode("ascii"),
    }

    async with httpx.AsyncClient(timeout=90.0) as client:
        response = await client.post(
            url,
            headers={"Authorization": f"Bearer {token}"},
            json=payload,
        )

    if response.status_code >= 400:
        raise RuntimeError(f"GCP Speech error ({response.status_code}): {response.text}")

    data = response.json()
    parts: list[str] = []
    for result in data.get("results") or []:
        alts = result.get("alternatives") or []
        if alts and alts[0].get("transcript"):
            text = alts[0]["transcript"].strip()
            if text:
                parts.append(text)
    text = " ".join(parts).strip()
    if not text:
        raise RuntimeError("GCP Speech returned empty text (no speech detected).")
    return text
