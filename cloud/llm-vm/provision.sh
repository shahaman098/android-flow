#!/usr/bin/env bash
# Create Flow open-weight LLM VM (Qwen2.5-3B via Ollama) on MyGCP.
# Note: project GPU quota is currently 0 — uses n2-standard-8 CPU. Upgrade to L4 when quota allows.
set -euo pipefail

ACCOUNT="sahkris0844@gmail.com"
PROJECT="project-ced3b331-e814-4d72-8bc"
ZONE="europe-west2-a"
NAME="flow-llm"
MACHINE="n2-standard-8"
DISK_GB=100
ROOT="$(cd "$(dirname "$0")" && pwd)"
RUN_NETWORK_TAG="flow-run"

gcloud config set account "$ACCOUNT"
gcloud config set project "$PROJECT"

API_KEY=$(gcloud secrets versions access latest --secret=hermes-api-server-key --project="$PROJECT")

echo "==> Firewall for Flow LLM proxy (:8080)"
if ! gcloud compute firewall-rules describe allow-flow-llm-8080 --project="$PROJECT" >/dev/null 2>&1; then
  gcloud compute firewall-rules create allow-flow-llm-8080 \
    --project="$PROJECT" \
    --allow=tcp:8080 \
    --source-ranges= \
    --source-tags="$RUN_NETWORK_TAG" \
    --target-tags=flow-llm \
    --description="Flow Cloud Run to private LLM proxy"
else
  gcloud compute firewall-rules update allow-flow-llm-8080 \
    --project="$PROJECT" \
    --allow=tcp:8080 \
    --source-tags="$RUN_NETWORK_TAG" \
    --target-tags=flow-llm \
    --quiet
fi

echo "==> Create / update VM ${NAME}"
if gcloud compute instances describe "$NAME" --zone="$ZONE" --project="$PROJECT" >/dev/null 2>&1; then
  echo "VM exists — refreshing metadata + startup script"
  gcloud compute instances add-metadata "$NAME" \
    --zone="$ZONE" \
    --project="$PROJECT" \
    --metadata-from-file=startup-script="${ROOT}/startup.sh" \
    --metadata="llm-api-key=${API_KEY}"
  gcloud compute instances reset "$NAME" --zone="$ZONE" --project="$PROJECT" --quiet
else
  gcloud compute instances create "$NAME" \
    --project="$PROJECT" \
    --zone="$ZONE" \
    --machine-type="$MACHINE" \
    --boot-disk-size="${DISK_GB}GB" \
    --boot-disk-type=pd-balanced \
    --image-family=ubuntu-2204-lts \
    --image-project=ubuntu-os-cloud \
    --tags=flow-llm \
    --scopes=cloud-platform \
    --metadata="llm-api-key=${API_KEY}" \
    --metadata-from-file=startup-script="${ROOT}/startup.sh"
fi

IP=$(gcloud compute instances describe "$NAME" --zone="$ZONE" --project="$PROJECT" \
  --format='get(networkInterfaces[0].networkIP)')
echo "VM private IP: $IP"
echo "Waiting for model pull + proxy (can take 10–20 min on first boot)…"

for i in $(seq 1 90); do
  if gcloud compute ssh "$NAME" --zone="$ZONE" --project="$PROJECT" --quiet \
      --command='curl -sf http://127.0.0.1:8080/health' >/tmp/flow-llm-health.json 2>/dev/null; then
    echo "Health OK:"
    cat /tmp/flow-llm-health.json
    echo
    break
  fi
  echo "  attempt $i… not ready"
  sleep 20
done

echo ""
echo "LLM_BASE_URL=http://${IP}:8080"
echo "LLM_MODEL=qwen2.5:3b"
echo "Next: run cloud/deploy.sh to attach Cloud Run through the private VPC."
