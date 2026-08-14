# Cloud hosting notes — Android Flow

Last updated: 2026-08-14

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

- Android app captures mic audio / focused text and handles insert only.
- Cloud Run service `flow-api` runs with `min-instances=0`, `512Mi`, `1 vCPU`, `max=2`.
- Cloud STT defaults to Groq `whisper-large-v3-turbo`.
- Cloud LLM defaults to DeepSeek `deepseek-v4-flash`.
- No Compute Engine `flow-llm` VM, no VPC connector, and no always-on Cloud Run instance.

Required env values before deploying:

```text
GROQ_API_KEY=...
DEEPSEEK_API_KEY=...
```

Deploy:

```bash
bash cloud/deploy.sh
```

Paste the resulting `FLOW_API_URL` and `FLOW_API_KEY` into the Android Hub settings.

To use GCP Speech instead of Groq, set `STT_PROVIDER=gcp_speech` before deploy. That is an
explicit provider choice (more expensive), not an automatic fallback.

## Recreate cheap Flow cloud hosting

Only do this after intentionally re-enabling billing for the project.

1. Re-enable billing for `project-ced3b331-e814-4d72-8bc` in Google Cloud Console.
2. Confirm gcloud identity (commands above).
3. Export `GROQ_API_KEY` and `DEEPSEEK_API_KEY`.
4. `bash cloud/deploy.sh`
5. Confirm health:

```bash
curl -H "Authorization: Bearer $FLOW_API_KEY" "$FLOW_API_URL/health"
```

6. Enter URL + key in the Android Hub → Save → Test API → Launch bubble.

## Cost controls

- Keep Cloud Run `min-instances=0` unless you need warm instances.
- Do not run `flow-llm` 24/7 for personal use unless the monthly cost is acceptable.
- Check resources:

```bash
gcloud run services list --region=europe-west2
gcloud billing projects describe project-ced3b331-e814-4d72-8bc
```

## Important implementation note

`cloud/app/llm.py` contains a conservative note-preservation guard. Deploy the current repo
version so cloud-mode auto-correction does not rewrite notes aggressively.
