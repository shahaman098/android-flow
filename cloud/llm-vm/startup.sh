#!/bin/bash
# Bootstraps open-weight Qwen2.5-3B (Ollama) + Bearer auth proxy for Flow.
set -euo pipefail
exec > >(tee -a /var/log/flow-llm-startup.log) 2>&1

apt-get update -y
DEBIAN_FRONTEND=noninteractive apt-get install -y curl ca-certificates python3 python3-pip python3-venv

# --- Ollama (localhost only) ---
if ! command -v ollama >/dev/null 2>&1; then
  curl -fsSL https://ollama.com/install.sh | sh
fi

mkdir -p /etc/systemd/system/ollama.service.d
cat >/etc/systemd/system/ollama.service.d/override.conf <<'EOF'
[Service]
Environment="OLLAMA_HOST=127.0.0.1:11434"
Environment="OLLAMA_KEEP_ALIVE=30m"
Environment="OLLAMA_NUM_PARALLEL=1"
EOF
systemctl daemon-reload
systemctl enable ollama
systemctl restart ollama

until curl -sf http://127.0.0.1:11434/api/tags >/dev/null; do sleep 2; done

# Multidisciplinary open-weight instruct model (coding, writing, reasoning)
# Must set HOME — ollama panics otherwise when run from systemd/root startup.
sudo -u ollama -H bash -c 'export HOME=/usr/share/ollama; ollama pull qwen2.5:3b'

# --- API key from instance metadata ---
META_KEY=$(curl -sf -H "Metadata-Flavor: Google" \
  http://metadata.google.internal/computeMetadata/v1/instance/attributes/llm-api-key || true)
if [[ -n "${META_KEY}" ]]; then
  printf '%s' "$META_KEY" >/etc/flow-llm-api-key
  chmod 600 /etc/flow-llm-api-key
  echo "LLM_API_KEY=${META_KEY}" >/etc/flow-llm-env
  chmod 600 /etc/flow-llm-env
fi

# --- Auth proxy ---
python3 -m venv /opt/flow-llm-venv
/opt/flow-llm-venv/bin/pip install -q fastapi uvicorn httpx

cat >/opt/flow_llm_proxy.py <<'PY'
import os
import httpx
from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import Response

OLLAMA = os.environ.get("OLLAMA_BASE", "http://127.0.0.1:11434")
API_KEY = os.environ.get("LLM_API_KEY", "").strip()
DEFAULT_MODEL = os.environ.get("LLM_MODEL", "qwen2.5:3b")

app = FastAPI(title="Flow open-weight LLM proxy")


def check_auth(authorization: str | None) -> None:
    if not API_KEY:
        return
    if not authorization or not authorization.lower().startswith("bearer "):
        raise HTTPException(status_code=401, detail="Missing Bearer token")
    if authorization.split(" ", 1)[1].strip() != API_KEY:
        raise HTTPException(status_code=401, detail="Invalid API key")


@app.get("/health")
async def health():
    async with httpx.AsyncClient(timeout=10) as client:
        r = await client.get(f"{OLLAMA}/api/tags")
    models = [m.get("name") for m in (r.json().get("models") or [])]
    return {
        "ok": True,
        "backend": "ollama",
        "model_default": DEFAULT_MODEL,
        "models": models,
    }


@app.get("/v1/models")
async def models(authorization: str | None = Header(default=None)):
    check_auth(authorization)
    async with httpx.AsyncClient(timeout=30) as client:
        r = await client.get(f"{OLLAMA}/api/tags")
    data = [
        {"id": m.get("name"), "object": "model", "owned_by": "ollama"}
        for m in (r.json().get("models") or [])
    ]
    if not data:
        data = [{"id": DEFAULT_MODEL, "object": "model", "owned_by": "ollama"}]
    return {"object": "list", "data": data}


@app.post("/v1/chat/completions")
async def chat(request: Request, authorization: str | None = Header(default=None)):
    check_auth(authorization)
    body = await request.json()
    body.setdefault("model", DEFAULT_MODEL)
    body["stream"] = False
    async with httpx.AsyncClient(timeout=550.0) as client:
        r = await client.post(f"{OLLAMA}/v1/chat/completions", json=body)
    return Response(
        content=r.content,
        status_code=r.status_code,
        media_type="application/json",
    )
PY

cat >/etc/systemd/system/flow-llm-proxy.service <<'EOF'
[Unit]
Description=Flow open-weight LLM auth proxy
After=network-online.target ollama.service
Wants=network-online.target ollama.service

[Service]
Type=simple
Environment=OLLAMA_BASE=http://127.0.0.1:11434
Environment=LLM_MODEL=qwen2.5:3b
EnvironmentFile=-/etc/flow-llm-env
ExecStart=/opt/flow-llm-venv/bin/uvicorn flow_llm_proxy:app --host 0.0.0.0 --port 8080 --app-dir /opt
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable flow-llm-proxy
systemctl restart flow-llm-proxy
echo "Flow LLM ready: qwen2.5:3b on private :8080"
