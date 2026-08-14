# Cloud hosting notes

Last updated: 2026-08-13

Use this note when recreating Flow cloud hosting. The previous cloud stack was intentionally
removed to stop charges.

## GCP identity

- Account: `sahkris0844@gmail.com`
- Project ID: `project-ced3b331-e814-4d72-8bc`
- Project number: `597773359205`
- Region: `europe-west2`
- Zone used by the old VM: `europe-west2-a`
- Billing account used before teardown: `01339D-E5A0DD-627407`
- Do not use `amanshah0843@gmail.com` unless explicitly requested.

Connect with:

```bash
gcloud config set account sahkris0844@gmail.com
gcloud config set project project-ced3b331-e814-4d72-8bc
```

If credentials expire:

```bash
gcloud auth login sahkris0844@gmail.com
gcloud auth application-default login
```

## Current cloud state

Billing was unlinked from the project:

```text
billingAccountName: ''
billingEnabled: false
```

This prevents new billable Google Cloud usage from this project. Google may still post delayed
charges for usage that happened before billing was disabled.

## Cheapest recreated architecture

Use this for "cloud, but maximum cheap":

- Mac app captures mic audio and handles paste only.
- Cloud Run service `flow-api` runs with `min-instances=0`, `512Mi`, `1 vCPU`, `max=2`.
- Cloud STT defaults to Groq `whisper-large-v3-turbo`.
- Cloud LLM defaults to DeepSeek `deepseek-v4-flash`.
- No Compute Engine `flow-llm` VM, no VPC connector, and no always-on Cloud Run instance.

Required app `.env` values before deploying:

```text
GROQ_API_KEY=...
DEEPSEEK_API_KEY=...
```

Deploy:

```bash
bash cloud/deploy.sh
```

If Groq is not available, `STT_PROVIDER=gcp_speech bash cloud/deploy.sh` keeps processing in the
cloud but is materially more expensive for speech-to-text.

Deleted resources:

- Cloud Run services: `flow-api`, `boto-ai`, `clutch`, `n8n`
- Flow VM: `flow-llm`
- Flow VM disk: `flow-llm` `100GB pd-balanced`
- Flow firewall rule: `allow-flow-llm-8080`
- Flow secrets: `flow-api-key`, `hermes-api-server-key`
- Flow service account: `flow-runtime@project-ced3b331-e814-4d72-8bc.iam.gserviceaccount.com`

Stopped before billing was disabled:

- `hermes-agent` was stopped. It was not deleted because it was not explicitly part of Flow.
  After billing was disabled, GCP API calls that require billing may fail until billing is
  re-enabled.

## Previous Flow cloud architecture

- Mac app captured mic audio and keypresses only.
- Cloud Run service `flow-api` exposed:
  - `POST /v1/transcribe`
  - `POST /v1/process`
  - `GET /health`
- Auth used bearer `FLOW_API_KEY` from Secret Manager secret `flow-api-key`.
- STT used Google Cloud Speech-to-Text V2:
  - Region: `europe-west2`
  - Model: `latest_long`
  - Default language: `en-GB`
- LLM used Ollama behind an auth proxy on `flow-llm`:
  - VM: `n2-standard-8`
  - Disk: `100GB pd-balanced`
  - Zone: `europe-west2-a`
  - Model at teardown: `qwen2.5:3b`
  - Private proxy URL used by Cloud Run: `http://10.154.0.5:8080`
- Cloud Run sizing before teardown:
  - `1 vCPU`
  - `1Gi`
  - `max=3`
  - Previously `min=1`, which caused idle monthly charges.

## Recreate cheap Flow cloud hosting

Only do this after intentionally re-enabling billing for the project.

1. Re-enable billing for `project-ced3b331-e814-4d72-8bc` in Google Cloud Console.
2. Confirm gcloud identity:

```bash
gcloud config set account sahkris0844@gmail.com
gcloud config set project project-ced3b331-e814-4d72-8bc
gcloud auth list
gcloud config list
```

3. Add `GROQ_API_KEY` and `DEEPSEEK_API_KEY` to
   `~/Library/Application Support/voice-flow/.env`, or export them in the shell.
4. Deploy Cloud Run:

```bash
bash cloud/deploy.sh
```

5. Confirm health:

```bash
curl -H "Authorization: Bearer $FLOW_API_KEY" "$FLOW_API_URL/health"
```

6. Confirm the Mac app config points to the new Cloud Run URL and secret:

```text
~/Library/Application Support/voice-flow/.env
FLOW_API_URL=...
FLOW_API_KEY=...
PROCESSING_MODE=cloud
```

## Legacy VM recreation

Only use this if explicitly choosing the more expensive open-weight VM architecture:

```bash
bash cloud/llm-vm/provision.sh
```

## Cost controls for any future cloud host

- Keep Cloud Run `min-instances=0` unless there is a clear reason to pay for warm idle instances.
- Do not run `flow-llm` 24/7 for personal use unless the monthly cost is acceptable.
- Prefer explicit start/stop commands for the VM:

```bash
gcloud compute instances start flow-llm --zone=europe-west2-a
gcloud compute instances stop flow-llm --zone=europe-west2-a
```

- Check current resources before assuming what is running:

```bash
gcloud compute instances list
gcloud compute disks list
gcloud run services list --region=europe-west2
gcloud billing projects describe project-ced3b331-e814-4d72-8bc
```

- If credits are low, disable billing or delete the cloud stack before the credit expires.

## Important implementation note

The cloud app currently contains a conservative note-preservation guard in `cloud/app/llm.py`.
If cloud hosting is recreated, deploy the current repo version so cloud-mode auto-correction does
not rewrite notes aggressively.
