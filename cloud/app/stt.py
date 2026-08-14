"""Cloud speech-to-text providers for Flow."""

from __future__ import annotations

import base64
import re

import google.auth
import google.auth.transport.requests
import httpx

from .settings import settings

_ARTIFACT_RE = re.compile(
    r"[\[(]\s*(?:BLANK[_\s-]?AUDIO|SILENCE|MUSIC|INAUDIBLE|NOISE|APPLAUSE|LAUGHTER|COUGH|UNKNOWN)\s*[\])]",
    re.IGNORECASE,
)
_PROMPT_LEAK_RE = re.compile(
    r"^\s*(?:Accurate dictation transcript|Preserve product names, hotkeys, and technical terms exactly|Spell these product names and terms exactly)\s*[.:]?\s*",
    re.IGNORECASE,
)


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
    raw = lang.lower().replace("_", "-")
    if raw in ("", "auto", "en"):
        return "en-GB"
    if raw in ("en-us",):
        return "en-US"
    if raw in ("en-gb",):
        return "en-GB"
    if raw in ("hi", "hi-in"):
        return "hi-IN"
    if raw in ("ne", "ne-np"):
        return "ne-NP"
    if "-" in lang:
        return lang
    return lang


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
    provider = settings.stt_provider.strip().lower()
    if provider in ("", "groq", "groq_whisper"):
        return await _transcribe_groq(audio_bytes, language, dictionary)
    if provider == "gcp_speech":
        return await _transcribe_gcp(audio_bytes, language, dictionary)
    raise ValueError(f"Unsupported STT_PROVIDER: {settings.stt_provider}")


async def health() -> dict:
    provider = settings.stt_provider.strip().lower()
    if provider in ("", "groq", "groq_whisper"):
        key = settings.groq_api_key.strip()
        if not key:
            raise RuntimeError("Groq API key is not configured (GROQ_API_KEY).")
        async with httpx.AsyncClient(timeout=15.0) as client:
            response = await client.get(
                "https://api.groq.com/openai/v1/models",
                headers={"Authorization": f"Bearer {key}"},
            )
        if response.status_code >= 400:
            raise RuntimeError(f"Groq health error ({response.status_code}): {response.text}")
        return {"ok": True, "provider": "groq_whisper", "model": settings.groq_stt_model}

    if provider == "gcp_speech":
        _access_token()
        return {"ok": True, "provider": "gcp_speech", "model": settings.stt_model}

    raise ValueError(f"Unsupported STT_PROVIDER: {settings.stt_provider}")


async def _transcribe_gcp(
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
    text = sanitize_transcript(" ".join(parts))
    if not text:
        raise RuntimeError("GCP Speech returned empty text (no speech detected).")
    return text


def _groq_language(language: str | None) -> str | None:
    lang = (language or settings.default_language).strip().lower()
    if lang in ("", "auto"):
        return None
    if "-" in lang:
        return lang.split("-", 1)[0]
    if "_" in lang:
        return lang.split("_", 1)[0]
    return lang


def _groq_prompt(dictionary: list[str] | None) -> str | None:
    words = [w.strip() for w in (dictionary or []) if w.strip()]
    if not words:
        return None
    # Vocabulary only — instructional sentences get leaked into the transcript
    # when Whisper hears silence (e.g. "Accurate dictation transcript.[BLANK_AUDIO]").
    return ", ".join(words[:60])[:900]


async def _transcribe_groq(
    audio_bytes: bytes,
    language: str | None = None,
    dictionary: list[str] | None = None,
) -> str:
    if len(audio_bytes) < 1500:
        raise ValueError(f"Audio too short ({len(audio_bytes)} bytes).")

    key = settings.groq_api_key.strip()
    if not key:
        raise RuntimeError("Groq API key is not configured (GROQ_API_KEY).")

    data = {
        "model": settings.groq_stt_model or "whisper-large-v3-turbo",
        "response_format": "json",
        "temperature": "0",
    }
    lang = _groq_language(language)
    if lang:
        data["language"] = lang
    prompt = _groq_prompt(dictionary)
    if prompt:
        data["prompt"] = prompt

    files = {
        "file": (
            "flow-dictation.webm",
            audio_bytes,
            "audio/webm",
        )
    }
    async with httpx.AsyncClient(timeout=90.0) as client:
        response = await client.post(
            "https://api.groq.com/openai/v1/audio/transcriptions",
            headers={"Authorization": f"Bearer {key}"},
            data=data,
            files=files,
        )

    if response.status_code >= 400:
        raise RuntimeError(f"Groq Speech error ({response.status_code}): {response.text}")

    text = sanitize_transcript(response.json().get("text") or "")
    if not text:
        raise RuntimeError("Groq Speech returned empty text (no speech detected).")
    return text


def sanitize_transcript(text: str) -> str:
    """Drop Whisper prompt leaks and [BLANK_AUDIO]-style tags."""
    cleaned = _ARTIFACT_RE.sub(" ", text or "")
    cleaned = _PROMPT_LEAK_RE.sub("", cleaned)
    cleaned = " ".join(cleaned.split())
    if cleaned.lower().startswith("vocabulary:"):
        return ""
    return cleaned
