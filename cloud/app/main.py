"""Flow processing API — STT + grammar + Vibe prompts on MyGCP Cloud Run."""

from __future__ import annotations

import base64

from fastapi import Depends, FastAPI, HTTPException
from pydantic import BaseModel, Field

from . import llm, stt
from .auth import require_api_key
from .settings import settings

app = FastAPI(title="Flow Processing API", version="1.0.0")


class TranscribeRequest(BaseModel):
    audio_base64: str
    language: str | None = None
    dictionary: list[str] = Field(default_factory=list)


class ProcessRequest(BaseModel):
    mode: str = Field(description="vibe | vibe_refine | dictate")
    audio_base64: str | None = None
    selected_text: str | None = None
    project_context: str | None = None
    constitution: str | None = None
    skill: str | None = None
    language: str | None = None
    dictionary: list[str] = Field(default_factory=list)


class TextResponse(BaseModel):
    text: str
    raw_transcript: str | None = None


class MeetingTranscribeRequest(BaseModel):
    audio_base64: str
    speaker: str = Field(description="'you' (mic) or 'other' (system audio)")
    session_id: str
    language: str | None = None
    dictionary: list[str] = Field(default_factory=list)


class MeetingTranscribeResponse(BaseModel):
    text: str
    speaker: str


@app.get("/")
async def root() -> dict:
    """Browser-friendly landing (avoids bare FastAPI {\"detail\":\"Not Found\"})."""
    return {
        "service": "Flow Processing API",
        "ok": True,
        "project": settings.gcp_project_id,
        "region": settings.gcp_location,
        "docs": "/docs",
        "health": "/health",
        "endpoints": {
            "transcribe": "POST /v1/transcribe",
            "process": "POST /v1/process",
        },
        "note": "Mac Flow.app calls these with Bearer FLOW_API_KEY. Opening / in a browser is fine.",
    }


@app.get("/health")
async def health() -> dict:
    try:
        llm_status = await llm.health()
    except Exception as exc:  # noqa: BLE001 - health must expose downstream failure
        raise HTTPException(status_code=503, detail=f"LLM backend unavailable: {exc}") from exc
    return {
        "ok": True,
        "project": settings.gcp_project_id,
        "region": settings.gcp_location,
        "stt_model": settings.stt_model,
        "llm_backend": "ollama",
        "llm_url": settings.hermes_base_url,
        "llm_model": settings.hermes_model,
        "llm_ok": llm_status["ok"],
    }


@app.post("/v1/transcribe", response_model=TextResponse, dependencies=[Depends(require_api_key)])
async def transcribe(body: TranscribeRequest) -> TextResponse:
    try:
        audio = base64.b64decode(body.audio_base64.strip())
        text = await stt.transcribe(audio, body.language, body.dictionary)
        return TextResponse(text=text, raw_transcript=text)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except Exception as exc:  # noqa: BLE001 — surface to Mac client
        raise HTTPException(status_code=502, detail=str(exc)) from exc


@app.post("/v1/process", response_model=TextResponse, dependencies=[Depends(require_api_key)])
async def process(body: ProcessRequest) -> TextResponse:
    mode = body.mode.strip().lower()
    language = body.language or settings.default_language

    try:
        if mode == "vibe_refine":
            selected = (body.selected_text or "").strip()
            if not selected:
                raise HTTPException(
                    status_code=400,
                    detail="vibe_refine requires selected_text (Control+2).",
                )
            text = await llm.refine_prompt(
                selected,
                body.project_context or "",
                body.constitution or "",
                language,
                body.dictionary,
                body.skill or "",
            )
            return TextResponse(text=text)

        if mode not in ("vibe", "dictate"):
            raise HTTPException(status_code=400, detail=f"Unsupported mode: {body.mode}")

        if not body.audio_base64 or not body.audio_base64.strip():
            raise HTTPException(status_code=400, detail=f"{mode} mode requires audio_base64.")

        audio = base64.b64decode(body.audio_base64.strip())
        raw = await stt.transcribe(audio, language, body.dictionary)
        corrected = await llm.polish(raw, language, body.dictionary)
        if mode == "dictate":
            # Hold § — activate: grammar-cleaned text only (no prompt generation).
            return TextResponse(text=corrected, raw_transcript=raw)
        text = await llm.vibe_prompt(
            corrected, body.constitution or "", language, body.dictionary, body.skill or ""
        )
        return TextResponse(text=text, raw_transcript=raw)
    except HTTPException:
        raise
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=502, detail=str(exc)) from exc


@app.post(
    "/v1/meeting/transcribe",
    response_model=MeetingTranscribeResponse,
    dependencies=[Depends(require_api_key)],
)
async def meeting_transcribe(body: MeetingTranscribeRequest) -> MeetingTranscribeResponse:
    """Transcribes one ~20s meeting-audio chunk. No grammar/LLM polish — raw STT text only,
    matching the meeting-transcript feature's transcript-only scope (no summarization step)."""
    try:
        audio = base64.b64decode(body.audio_base64.strip())
        text = await stt.transcribe(audio, body.language, body.dictionary)
        return MeetingTranscribeResponse(text=text, speaker=body.speaker)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    except Exception as exc:  # noqa: BLE001
        raise HTTPException(status_code=502, detail=str(exc)) from exc
