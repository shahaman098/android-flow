#!/usr/bin/env bash
# Deploy Flow processing API to MyGCP (n8n App / project-ced3b331-e814-4d72-8bc).
set -euo pipefail

ACCOUNT="sahkris0844@gmail.com"
PROJECT="project-ced3b331-e814-4d72-8bc"
REGION="europe-west2"
ZONE="europe-west2-a"
SERVICE="flow-api"
SA_NAME="flow-runtime"
SA_EMAIL="${SA_NAME}@${PROJECT}.iam.gserviceaccount.com"
LLM_VM="flow-llm"
NETWORK="default"
SUBNET="default"
RUN_NETWORK_TAG="flow-run"
LLM_NETWORK_TAG="flow-llm"
LLM_FIREWALL_RULE="allow-flow-llm-8080"
ROOT="$(cd "$(dirname "$0")" && pwd)"

gcloud config set account "$ACCOUNT"
gcloud config set project "$PROJECT"

echo "==> Enable APIs"
gcloud services enable \
  run.googleapis.com \
  cloudbuild.googleapis.com \
  artifactregistry.googleapis.com \
  compute.googleapis.com \
  speech.googleapis.com \
  secretmanager.googleapis.com \
  --project="$PROJECT"

echo "==> Service account"
if ! gcloud iam service-accounts describe "$SA_EMAIL" --project="$PROJECT" >/dev/null 2>&1; then
  gcloud iam service-accounts create "$SA_NAME" \
    --display-name="Flow processing runtime" \
    --project="$PROJECT"
fi

gcloud projects add-iam-policy-binding "$PROJECT" \
  --member="serviceAccount:${SA_EMAIL}" \
  --role="roles/speech.client" \
  --condition=None >/dev/null
echo "==> Secrets"
if ! gcloud secrets describe flow-api-key --project="$PROJECT" >/dev/null 2>&1; then
  KEY=$(openssl rand -hex 32)
  printf '%s' "$KEY" | gcloud secrets create flow-api-key \
    --project="$PROJECT" \
    --replication-policy=automatic \
    --data-file=-
  echo "Created secret flow-api-key"
else
  KEY=$(gcloud secrets versions access latest --secret=flow-api-key --project="$PROJECT")
  echo "Using existing secret flow-api-key"
fi

# Allow runtime SA to read secrets
for SECRET in flow-api-key hermes-api-server-key; do
  gcloud secrets add-iam-policy-binding "$SECRET" \
    --project="$PROJECT" \
    --member="serviceAccount:${SA_EMAIL}" \
    --role="roles/secretmanager.secretAccessor" \
    --condition=None >/dev/null 2>&1 || true
done

# Secret access is granted on the two Flow secrets above; remove the older
# project-wide grant if a previous deployment created it.
gcloud projects remove-iam-policy-binding "$PROJECT" \
  --member="serviceAccount:${SA_EMAIL}" \
  --role="roles/secretmanager.secretAccessor" \
  --condition=None >/dev/null 2>&1 || true

LLM_PRIVATE_IP=$(gcloud compute instances describe "$LLM_VM" \
  --zone="$ZONE" \
  --project="$PROJECT" \
  --format='get(networkInterfaces[0].networkIP)')
if [[ -z "$LLM_PRIVATE_IP" ]]; then
  echo "error: could not resolve private IP for ${LLM_VM}" >&2
  exit 1
fi

# min=1: scale-to-zero put a ~4s cold start in front of the first
# dictation after any idle gap, which is the one the user notices most. One warm
# instance at 1 vCPU / 1Gi is a rounding error next to the always-on flow-llm VM.
echo "==> Deploy Cloud Run service ${SERVICE}"
gcloud run deploy "$SERVICE" \
  --source="$ROOT" \
  --project="$PROJECT" \
  --region="$REGION" \
  --service-account="$SA_EMAIL" \
  --allow-unauthenticated \
  --quiet \
  --memory=1Gi \
  --cpu=1 \
  --timeout=1100 \
  --max=3 \
  --min=1 \
  --network="$NETWORK" \
  --subnet="$SUBNET" \
  --network-tags="$RUN_NETWORK_TAG" \
  --vpc-egress=private-ranges-only \
  --set-env-vars="GCP_PROJECT_ID=${PROJECT},GCP_LOCATION=${REGION},STT_MODEL=latest_long,DEFAULT_LANGUAGE=en-GB,HERMES_BASE_URL=http://${LLM_PRIVATE_IP}:8080,HERMES_MODEL=qwen2.5:3b" \
  --set-secrets="FLOW_API_KEY=flow-api-key:latest,HERMES_API_KEY=hermes-api-server-key:latest"

URL=$(gcloud run services describe "$SERVICE" --project="$PROJECT" --region="$REGION" --format='value(status.url)')
echo ""
echo "DEPLOYED: $URL"
echo "FLOW_API_URL=$URL"
echo "FLOW_API_KEY=<from Secret Manager flow-api-key>"

# Write local Mac env (gitignored Application Support path)
APP_SUPPORT="${HOME}/Library/Application Support/voice-flow"
mkdir -p "$APP_SUPPORT"
export FLOW_DEPLOY_URL="$URL"
export FLOW_DEPLOY_KEY="$KEY"
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

echo "==> Health check over private VPC path"
curl --fail --silent --show-error --max-time 30 "${URL}/health"
echo ""

echo "==> Restrict LLM proxy to Cloud Run's VPC network tag"
if gcloud compute firewall-rules describe "$LLM_FIREWALL_RULE" --project="$PROJECT" >/dev/null 2>&1; then
  gcloud compute firewall-rules update "$LLM_FIREWALL_RULE" \
    --project="$PROJECT" \
    --allow=tcp:8080 \
    --source-ranges= \
    --source-tags="$RUN_NETWORK_TAG" \
    --target-tags="$LLM_NETWORK_TAG" \
    --quiet
else
  gcloud compute firewall-rules create "$LLM_FIREWALL_RULE" \
    --project="$PROJECT" \
    --allow=tcp:8080 \
    --source-tags="$RUN_NETWORK_TAG" \
    --target-tags="$LLM_NETWORK_TAG" \
    --description="Flow Cloud Run to private LLM proxy" \
    --quiet
fi

echo "==> Post-firewall health check"
curl --fail --silent --show-error --max-time 30 "${URL}/health"
echo ""
echo "DONE"
