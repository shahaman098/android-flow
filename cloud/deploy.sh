#!/usr/bin/env bash
# Deploy Flow processing API to the cheapest cloud shape:
# Cloud Run scale-to-zero + external pay-per-use STT/LLM APIs. No LLM VM.
set -euo pipefail

ACCOUNT="${GCP_ACCOUNT:-sahkris0844@gmail.com}"
PROJECT="${GCP_PROJECT:-project-ced3b331-e814-4d72-8bc}"
REGION="${GCP_REGION:-europe-west2}"
SERVICE="${FLOW_SERVICE:-flow-api}"
SA_NAME="${FLOW_SA_NAME:-flow-runtime}"
SA_EMAIL="${SA_NAME}@${PROJECT}.iam.gserviceaccount.com"
ROOT="$(cd "$(dirname "$0")" && pwd)"
APP_ENV="${HOME}/Library/Application Support/voice-flow/.env"

STT_PROVIDER="${STT_PROVIDER:-groq_whisper}"
GROQ_STT_MODEL="${GROQ_STT_MODEL:-whisper-large-v3-turbo}"
LLM_PROVIDER="${LLM_PROVIDER:-deepseek}"

case "$LLM_PROVIDER" in
  deepseek|"")
    LLM_PROVIDER="deepseek"
    LLM_BASE_URL="${LLM_BASE_URL:-${DEEPSEEK_BASE_URL:-https://api.deepseek.com}}"
    LLM_MODEL="${LLM_MODEL:-${DEEPSEEK_MODEL:-deepseek-v4-flash}}"
    LLM_KEY_ENV="DEEPSEEK_API_KEY"
    LLM_SECRET="deepseek-api-key"
    ;;
  xai)
    LLM_BASE_URL="${LLM_BASE_URL:-${XAI_BASE_URL:-https://api.x.ai/v1}}"
    LLM_MODEL="${LLM_MODEL:-${XAI_MODEL:-grok-4.3}}"
    LLM_KEY_ENV="XAI_API_KEY"
    LLM_SECRET="xai-api-key"
    ;;
  openai_compatible)
    LLM_BASE_URL="${LLM_BASE_URL:?LLM_BASE_URL is required for LLM_PROVIDER=openai_compatible}"
    LLM_MODEL="${LLM_MODEL:?LLM_MODEL is required for LLM_PROVIDER=openai_compatible}"
    LLM_KEY_ENV="LLM_API_KEY"
    LLM_SECRET="llm-api-key"
    ;;
  *)
    echo "error: unsupported LLM_PROVIDER=${LLM_PROVIDER}. Use deepseek, xai, or openai_compatible." >&2
    exit 1
    ;;
esac

read_local_env() {
  local name="$1"
  local value="${!name:-}"
  if [[ -n "$value" || ! -f "$APP_ENV" ]]; then
    printf '%s' "$value"
    return
  fi
  python3 - "$APP_ENV" "$name" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
name = sys.argv[2]
for raw in path.read_text().splitlines():
    line = raw.strip()
    if not line or line.startswith("#") or "=" not in line:
        continue
    key, value = line.split("=", 1)
    if key.strip() == name:
        print(value.strip().strip('"').strip("'"), end="")
        break
PY
}

ensure_secret() {
  local secret="$1"
  local env_name="$2"
  local required="$3"

  if gcloud secrets describe "$secret" --project="$PROJECT" >/dev/null 2>&1; then
    echo "Using existing secret ${secret}"
    return
  fi

  local value
  value="$(read_local_env "$env_name")"
  if [[ -z "$value" ]]; then
    if [[ "$required" == "required" ]]; then
      echo "error: ${env_name} is required to create Secret Manager secret ${secret}." >&2
      echo "Add it to ${APP_ENV} or export ${env_name}, then re-run this script." >&2
      exit 1
    fi
    return
  fi

  printf '%s' "$value" | gcloud secrets create "$secret" \
    --project="$PROJECT" \
    --replication-policy=automatic \
    --data-file=-
  echo "Created secret ${secret}"
}

grant_secret_access() {
  local secret="$1"
  if gcloud secrets describe "$secret" --project="$PROJECT" >/dev/null 2>&1; then
    gcloud secrets add-iam-policy-binding "$secret" \
      --project="$PROJECT" \
      --member="serviceAccount:${SA_EMAIL}" \
      --role="roles/secretmanager.secretAccessor" \
      --condition=None >/dev/null
  fi
}

gcloud config set account "$ACCOUNT"
gcloud config set project "$PROJECT"

echo "==> Enable APIs"
gcloud services enable \
  run.googleapis.com \
  cloudbuild.googleapis.com \
  artifactregistry.googleapis.com \
  secretmanager.googleapis.com \
  --project="$PROJECT"

if [[ "$STT_PROVIDER" == "gcp_speech" ]]; then
  gcloud services enable speech.googleapis.com --project="$PROJECT"
fi

echo "==> Service account"
if ! gcloud iam service-accounts describe "$SA_EMAIL" --project="$PROJECT" >/dev/null 2>&1; then
  gcloud iam service-accounts create "$SA_NAME" \
    --display-name="Flow processing runtime" \
    --project="$PROJECT"
fi

if [[ "$STT_PROVIDER" == "gcp_speech" ]]; then
  gcloud projects add-iam-policy-binding "$PROJECT" \
    --member="serviceAccount:${SA_EMAIL}" \
    --role="roles/speech.client" \
    --condition=None >/dev/null
fi

echo "==> Secrets"
if ! gcloud secrets describe flow-api-key --project="$PROJECT" >/dev/null 2>&1; then
  FLOW_KEY="$(openssl rand -hex 32)"
  printf '%s' "$FLOW_KEY" | gcloud secrets create flow-api-key \
    --project="$PROJECT" \
    --replication-policy=automatic \
    --data-file=-
  echo "Created secret flow-api-key"
else
  FLOW_KEY="$(gcloud secrets versions access latest --secret=flow-api-key --project="$PROJECT")"
  echo "Using existing secret flow-api-key"
fi

ensure_secret "$LLM_SECRET" "$LLM_KEY_ENV" required
if [[ "$STT_PROVIDER" == "groq" || "$STT_PROVIDER" == "groq_whisper" || -z "$STT_PROVIDER" ]]; then
  ensure_secret groq-api-key GROQ_API_KEY required
fi

grant_secret_access flow-api-key
grant_secret_access "$LLM_SECRET"
grant_secret_access groq-api-key

SECRET_ARGS="FLOW_API_KEY=flow-api-key:latest,LLM_API_KEY=${LLM_SECRET}:latest"
if gcloud secrets describe groq-api-key --project="$PROJECT" >/dev/null 2>&1; then
  SECRET_ARGS="${SECRET_ARGS},GROQ_API_KEY=groq-api-key:latest"
fi

echo "==> Deploy Cloud Run service ${SERVICE}"
gcloud run deploy "$SERVICE" \
  --source="$ROOT" \
  --project="$PROJECT" \
  --region="$REGION" \
  --service-account="$SA_EMAIL" \
  --allow-unauthenticated \
  --quiet \
  --memory=512Mi \
  --cpu=1 \
  --timeout=300 \
  --concurrency=4 \
  --max=2 \
  --min=0 \
  --set-env-vars="STT_PROVIDER=${STT_PROVIDER},GROQ_STT_MODEL=${GROQ_STT_MODEL},GCP_PROJECT_ID=${PROJECT},GCP_LOCATION=${REGION},STT_MODEL=latest_long,DEFAULT_LANGUAGE=en-GB,LLM_PROVIDER=${LLM_PROVIDER},LLM_BASE_URL=${LLM_BASE_URL},LLM_MODEL=${LLM_MODEL}" \
  --set-secrets="$SECRET_ARGS"

URL="$(gcloud run services describe "$SERVICE" --project="$PROJECT" --region="$REGION" --format='value(status.url)')"
echo ""
echo "DEPLOYED: $URL"
echo "FLOW_API_URL=$URL"
echo "FLOW_API_KEY=<from Secret Manager flow-api-key>"

echo "==> Write Mac app cloud config"
mkdir -p "$(dirname "$APP_ENV")"
export FLOW_DEPLOY_URL="$URL"
export FLOW_DEPLOY_KEY="$FLOW_KEY"
python3 <<'PY'
import os
from pathlib import Path

env_path = Path.home() / "Library/Application Support/voice-flow/.env"
url = os.environ["FLOW_DEPLOY_URL"]
key = os.environ["FLOW_DEPLOY_KEY"]
lines = []
if env_path.exists():
    for line in env_path.read_text().splitlines():
        if line.startswith(("FLOW_API_URL=", "FLOW_API_KEY=", "PROCESSING_MODE=")):
            continue
        lines.append(line)
lines.extend([
    f"FLOW_API_URL={url}",
    f"FLOW_API_KEY={key}",
    "PROCESSING_MODE=cloud",
])
env_path.write_text("\n".join(lines).rstrip() + "\n")
env_path.chmod(0o600)
env_path.parent.chmod(0o700)
print(f"Wrote {env_path} (FLOW_API_URL + FLOW_API_KEY + PROCESSING_MODE=cloud)")
PY

echo "==> Health check"
curl --fail --silent --show-error --max-time 30 -H "Authorization: Bearer ${FLOW_KEY}" "${URL}/health"
echo ""
echo "DONE"
