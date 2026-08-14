import hmac

from fastapi import Header, HTTPException

from .settings import settings


def require_api_key(authorization: str | None = Header(default=None)) -> None:
    expected = settings.flow_api_key.strip()
    if not expected:
        raise HTTPException(status_code=500, detail="FLOW_API_KEY is not configured on the server.")
    if not authorization or not authorization.lower().startswith("bearer "):
        raise HTTPException(status_code=401, detail="Missing Bearer token.")
    token = authorization.split(" ", 1)[1].strip()
    if not hmac.compare_digest(token, expected):
        raise HTTPException(status_code=401, detail="Invalid API key.")
