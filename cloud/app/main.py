"""Flow processing API — cloud STT + grammar + Vibe prompts on Cloud Run."""

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
    mode: str = Field(
        description="vibe_text | dictate | correct_text | meeting_answer | edit_text"
    )
    audio_base64: str | None = None
    selected_text: str | None = None
    project_context: str | None = None
    constitution: str | None = None
    skill: str | None = None
    language: str | None = None
    dictionary: list[str] = Field(default_factory=list)


class Mistake(BaseModel):
    type: str
    detail: str = ""


class ProcessTrace(BaseModel):
    draft: str | None = None
    repaired: bool = False
    used_fallback: bool = False
    polish_rejected: bool = False
    mistakes: list[Mistake] = Field(default_factory=list)


class TextResponse(BaseModel):
    text: str
    raw_transcript: str | None = None
    trace: ProcessTrace | None = None


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
        "docs": "/docs",
        "livez": "/livez",
        "health": "/health",
        "endpoints": {
            "transcribe": "POST /v1/transcribe",
            "process": "POST /v1/process",
        },
        "note": "Mac Flow.app calls these with Bearer FLOW_API_KEY. Opening / in a browser is fine.",
    }


@app.get("/livez")
async def livez() -> dict:
    return {"ok": True}


@app.get("/health", dependencies=[Depends(require_api_key)])
async def health() -> dict:
    try:
        llm_status = await llm.health()
        stt_status = await stt.health()
    except Exception as exc:  # noqa: BLE001 - health must expose downstream failure
        raise HTTPException(status_code=503, detail=f"Cloud backend unavailable: {exc}") from exc
    return {
        "ok": True,
        "project": settings.gcp_project_id,
        "region": settings.gcp_location,
        "stt_backend": stt_status["provider"],
        "stt_model": stt_status["model"],
        "llm_backend": llm_status["provider"],
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
        if mode == "vibe_text":
            selected = (body.selected_text or "").strip()
            if not selected:
                raise HTTPException(
                    status_code=400,
                    detail="vibe_text requires selected_text (fn+1).",
                )
            traced = await llm.vibe_prompt_traced(
                selected,
                body.project_context or "",
                body.constitution or "",
                language,
                body.dictionary,
                body.skill or "",
            )
            return TextResponse(text=traced.final, trace=ProcessTrace.model_validate(traced.as_api()))

        if mode == "correct_text":
            selected = (body.selected_text or "").strip()
            if not selected:
                raise HTTPException(
                    status_code=400,
                    detail="correct_text requires selected_text (fn+2).",
                )
            traced = await llm.polish_traced(selected, language, body.dictionary)
            return TextResponse(text=traced.final, trace=ProcessTrace.model_validate(traced.as_api()))

        if mode == "meeting_answer":
            question = (body.selected_text or "").strip()
            transcript = (body.project_context or "").strip()
            if not question or not transcript:
                raise HTTPException(
                    status_code=400,
                    detail="meeting_answer requires selected_text question and project_context transcript.",
                )
            traced = await llm.answer_meeting_question_traced(question, transcript, language)
            return TextResponse(text=traced.final, trace=ProcessTrace.model_validate(traced.as_api()))

        if mode == "edit_text":
            selected = (body.selected_text or "").strip()
            instruction = (body.project_context or "").strip()
            if not selected or not instruction:
                raise HTTPException(
                    status_code=400,
                    detail="edit_text requires selected_text and project_context instruction.",
                )
            text = await llm.edit_selected_text(selected, instruction, language, body.dictionary)
            return TextResponse(text=text)

        if mode != "dictate":
            raise HTTPException(status_code=400, detail=f"Unsupported mode: {body.mode}")

        if body.audio_base64 and body.audio_base64.strip():
            audio = base64.b64decode(body.audio_base64.strip())
            raw = await stt.transcribe(audio, language, body.dictionary)
        elif (body.selected_text or "").strip():
            raw = body.selected_text.strip()
        else:
            raise HTTPException(
                status_code=400,
                detail="dictate mode requires audio_base64 or selected_text transcript.",
            )
        traced = await llm.polish_traced(raw, language, body.dictionary)
        return TextResponse(
            text=traced.final,
            raw_transcript=raw,
            trace=ProcessTrace.model_validate(traced.as_api()),
        )
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
