#!/usr/bin/env bash
# Configure Flow.app for local use.
#
# Default: STT runs locally with whisper.cpp; cleanup/prompting uses the configured LLM API:
#   DEEPSEEK_API_KEY=... bash scripts/configure-local.sh
#
# Optional low-CPU STT API:
#   STT_PROVIDER=groq_whisper GROQ_API_KEY=... bash scripts/configure-local.sh
set -euo pipefail

APP_DIR="${HOME}/Library/Application Support/voice-flow"
APP_ENV="${APP_DIR}/.env"
APP_CONFIG="${APP_DIR}/config.json"
STT_PROVIDER="${STT_PROVIDER:-local_whisper}"
LOCAL_WHISPER_MODEL_PATH="${LOCAL_WHISPER_MODEL_PATH:-${APP_DIR}/models/ggml-small.en.bin}"
GROQ_STT_MODEL="${GROQ_STT_MODEL:-whisper-large-v3-turbo}"
LLM_PROVIDER="${LLM_PROVIDER:-deepseek}"

mkdir -p "$APP_DIR"
export APP_DIR APP_ENV APP_CONFIG STT_PROVIDER LOCAL_WHISPER_MODEL_PATH GROQ_STT_MODEL LLM_PROVIDER

python3 <<'PY'
import json
import os
import shutil
import sys
from pathlib import Path


app_dir = Path(os.environ["APP_DIR"])
env_path = Path(os.environ["APP_ENV"])
config_path = Path(os.environ["APP_CONFIG"])
stt_provider = os.environ["STT_PROVIDER"].strip() or "local_whisper"
local_model = Path(os.environ["LOCAL_WHISPER_MODEL_PATH"]).expanduser()
groq_model = os.environ["GROQ_STT_MODEL"].strip() or "whisper-large-v3-turbo"
llm_provider = os.environ["LLM_PROVIDER"].strip() or "deepseek"


def load_dotenv(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def load_config(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError as exc:
        raise SystemExit(f"error: {path} is not valid JSON: {exc}") from exc


dotenv = load_dotenv(env_path)
config = load_config(config_path)


def first_value(*names: str) -> str:
    for name in names:
        value = os.environ.get(name) or dotenv.get(name) or config.get(name.lower(), "")
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


if stt_provider in {"", "local", "local_whisper", "whisper_cpp"}:
    stt_provider = "local_whisper"
    missing = []
    if shutil.which("whisper-cli") is None:
        missing.append("whisper-cli")
    if shutil.which("ffmpeg") is None:
        missing.append("ffmpeg")
    if not local_model.exists():
        missing.append(str(local_model))
    if missing:
        details = ", ".join(missing)
        raise SystemExit(
            "error: local Whisper is not ready. Missing: "
            f"{details}\nInstall with: brew install whisper-cpp ffmpeg\n"
            "Download model: mkdir -p \"$HOME/Library/Application Support/voice-flow/models\" && "
            "curl -L https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin "
            "-o \"$HOME/Library/Application Support/voice-flow/models/ggml-small.en.bin\""
        )
elif stt_provider == "groq_whisper":
    if not first_value("GROQ_API_KEY"):
        raise SystemExit("error: GROQ_API_KEY is required when STT_PROVIDER=groq_whisper.")
else:
    raise SystemExit(
        f"error: unsupported STT_PROVIDER={stt_provider}. "
        "Use local_whisper or groq_whisper."
    )

if llm_provider == "deepseek":
    llm_base_url = first_value("LLM_BASE_URL", "DEEPSEEK_BASE_URL") or "https://api.deepseek.com"
    llm_model = first_value("LLM_MODEL", "DEEPSEEK_MODEL") or "deepseek-v4-flash"
    llm_key_name = "DEEPSEEK_API_KEY"
    llm_key = first_value("DEEPSEEK_API_KEY", "LLM_API_KEY")
elif llm_provider == "xai":
    llm_base_url = first_value("LLM_BASE_URL", "XAI_BASE_URL") or "https://api.x.ai/v1"
    llm_model = first_value("LLM_MODEL", "XAI_MODEL") or "grok-4.3"
    llm_key_name = "XAI_API_KEY"
    llm_key = first_value("XAI_API_KEY")
elif llm_provider == "openai_compatible":
    llm_base_url = first_value("LLM_BASE_URL")
    llm_model = first_value("LLM_MODEL")
    llm_key_name = "LLM_API_KEY"
    llm_key = first_value("LLM_API_KEY")
    if not llm_base_url or not llm_model:
        raise SystemExit(
            "error: LLM_BASE_URL and LLM_MODEL are required for "
            "LLM_PROVIDER=openai_compatible."
        )
else:
    raise SystemExit(
        f"error: unsupported LLM_PROVIDER={llm_provider}. "
        "Use deepseek, xai, or openai_compatible."
    )

if not llm_key:
    raise SystemExit(f"error: {llm_key_name} is required for local cleanup/prompting.")

config.update(
    {
        "processing_mode": "local",
        "stt_provider": stt_provider,
        "groq_stt_model": groq_model,
        "local_whisper_model_path": str(local_model),
        "llm_provider": llm_provider,
        "llm_base_url": llm_base_url,
        "llm_model": llm_model,
        "llm_api_key": llm_key,
        "correction_model": llm_model,
        "flow_api_url": "",
        "flow_api_key": "",
    }
)

groq_key = first_value("GROQ_API_KEY")
if groq_key:
    config["groq_api_key"] = groq_key

drop_prefixes = (
    "PROCESSING_MODE=",
    "STT_PROVIDER=",
    "LOCAL_WHISPER_MODEL_PATH=",
    "GROQ_API_KEY=",
    "GROQ_STT_MODEL=",
    "DEEPSEEK_API_KEY=",
    "XAI_API_KEY=",
    "LLM_API_KEY=",
    "LLM_PROVIDER=",
    "LLM_BASE_URL=",
    "DEEPSEEK_MODEL=",
    "XAI_MODEL=",
    "LLM_MODEL=",
    "FLOW_API_URL=",
    "FLOW_API_KEY=",
)
env_lines = []
if env_path.exists():
    for line in env_path.read_text().splitlines():
        if line.startswith(drop_prefixes):
            continue
        env_lines.append(line)

env_lines.extend(
    [
        "PROCESSING_MODE=local",
        f"STT_PROVIDER={stt_provider}",
        f"LOCAL_WHISPER_MODEL_PATH={local_model}",
        f"LLM_PROVIDER={llm_provider}",
        f"LLM_BASE_URL={llm_base_url}",
        f"LLM_MODEL={llm_model}",
        f"{llm_key_name}={llm_key}",
    ]
)
if groq_key:
    env_lines.append(f"GROQ_API_KEY={groq_key}")
env_lines.append(f"GROQ_STT_MODEL={groq_model}")

app_dir.mkdir(parents=True, exist_ok=True)
config_path.write_text(json.dumps(config, indent=2) + "\n")
env_path.write_text("\n".join(env_lines).rstrip() + "\n")
config_path.chmod(0o600)
env_path.chmod(0o600)
app_dir.chmod(0o700)

print("Configured Flow for local mode.")
print(f"STT provider: {stt_provider}")
if stt_provider == "local_whisper":
    print(f"Local Whisper model: {local_model}")
print(f"LLM provider: {llm_provider}")
print("Cloud Run URL/key: cleared")
PY
