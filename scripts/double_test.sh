#!/usr/bin/env bash
# Double-check / double-test Flow Vibe Coding pipeline (no secrets printed).
set -euo pipefail
ROOT="/Users/efi/Projects/Wispr Flow"
PASS=0
FAIL=0
round() { echo ""; echo "======== $1 ========"; }

ok() { echo "PASS: $1"; PASS=$((PASS+1)); }
bad() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }

round "1) Project structure"
for f in \
  constitutions/vibe-coding.md \
  context/project.md \
  skills/speech-to-text/SKILL.md \
  skills/grammar-correct/SKILL.md \
  skills/vibe-prompt/SKILL.md \
  skills/refine-prompt/SKILL.md \
  src-tauri/src/stt_gcp.rs \
  src-tauri/src/dictate.rs \
  src-tauri/src/vibe_context.rs \
  src-tauri/src/lib.rs
do
  if [[ -f "$ROOT/$f" ]]; then ok "exists $f"; else bad "missing $f"; fi
done

round "2) Local config + ADC (twice)"
for i in 1 2; do
  if gcloud auth application-default print-access-token >/dev/null 2>&1; then
    ok "ADC token round $i"
  else
    bad "ADC token round $i"
  fi
done
CFG="$HOME/Library/Application Support/voice-flow/config.json"
ENVF="$HOME/Library/Application Support/voice-flow/.env"
python3 - <<'PY'
import json, pathlib, sys
cfg=json.loads(pathlib.Path.home().joinpath("Library/Application Support/voice-flow/config.json").read_text())
checks=[
  (cfg.get("stt_provider")=="gcp_speech", "stt_provider"),
  (cfg.get("llm_provider")=="deepseek", "llm_provider"),
  (cfg.get("hotkey")=="Control+1", "hotkey Control+1"),
  (cfg.get("prompt_hotkey")=="Control+2", "prompt_hotkey Control+2"),
  (bool(cfg.get("llm_api_key")), "llm_api_key present"),
  (pathlib.Path(cfg.get("vibe_project_root") or "").joinpath("context").is_dir(), "vibe_project_root/context"),
]
env=pathlib.Path.home().joinpath("Library/Application Support/voice-flow/.env")
checks.append((env.exists() and "DEEPSEEK_API_KEY=" in env.read_text(), ".env DEEPSEEK_API_KEY"))
for okc, name in checks:
  print(("PASS" if okc else "FAIL")+f": config {name}")
  if not okc: sys.exit(2)
PY
ok "config integrity"

round "3) GCP Speech API verify (twice)"
TOKEN=$(gcloud auth application-default print-access-token)
PROJECT=project-ced3b331-e814-4d72-8bc
LOC=europe-west2
for i in 1 2; do
  CODE=$(curl -sS -o /tmp/speech_list_$i.json -w "%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    "https://${LOC}-speech.googleapis.com/v2/projects/${PROJECT}/locations/${LOC}/recognizers?pageSize=1")
  if [[ "$CODE" == "200" ]]; then ok "Speech list recognizers round $i (HTTP $CODE)"; else bad "Speech list round $i HTTP $CODE"; fi
done

round "4) GCP Speech recognize with real audio (twice)"
# Generate spoken sample via macOS say
say -o /tmp/flow_stt_test.aiff "Add a login button to the settings page"
# Prefer ffmpeg; fall back to afconvert
if command -v ffmpeg >/dev/null 2>&1; then
  ffmpeg -y -i /tmp/flow_stt_test.aiff -ar 16000 -ac 1 /tmp/flow_stt_test.wav >/dev/null 2>&1
else
  afconvert -f WAVE -d LEI16@16000 /tmp/flow_stt_test.aiff /tmp/flow_stt_test.wav
fi
B64=$(base64 -i /tmp/flow_stt_test.wav | tr -d '\n')
for i in 1 2; do
  CODE=$(curl -sS -o /tmp/speech_rec_$i.json -w "%{http_code}" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d "{\"config\":{\"autoDecodingConfig\":{},\"languageCodes\":[\"en-GB\"],\"model\":\"latest_long\"},\"content\":\"$B64\"}" \
    "https://${LOC}-speech.googleapis.com/v2/projects/${PROJECT}/locations/${LOC}/recognizers/_:recognize")
  TEXT=$(python3 - <<PY
import json
d=json.load(open("/tmp/speech_rec_$i.json"))
parts=[]
for r in d.get("results") or []:
  alts=r.get("alternatives") or []
  if alts and alts[0].get("transcript"):
    parts.append(alts[0]["transcript"].strip())
print(" ".join(parts))
PY
)
  if [[ "$CODE" == "200" && -n "$TEXT" ]]; then
    ok "Speech recognize round $i → ${TEXT:0:80}"
  else
    bad "Speech recognize round $i HTTP $CODE text='${TEXT:0:80}' body=$(head -c 200 /tmp/speech_rec_$i.json)"
  fi
done

round "5) DeepSeek grammar + vibe prompt + refine (twice each)"
KEY=$(grep '^DEEPSEEK_API_KEY=' "$ENVF" | head -1 | cut -d= -f2-)
CONTEXT=$(cat "$ROOT/context/project.md" "$ROOT/constitutions/vibe-coding.md")
for i in 1 2; do
  python3 - <<PY
import json, urllib.request, pathlib
key=open("$ENVF").read().split("DEEPSEEK_API_KEY=",1)[1].splitlines()[0].strip()
root=pathlib.Path("$ROOT")

def chat(messages, out_path):
    body=json.dumps({
        "model":"deepseek-v4-flash",
        "temperature":0.2,
        "thinking":{"type":"disabled"},
        "messages":messages,
    }).encode()
    req=urllib.request.Request(
        "https://api.deepseek.com/chat/completions",
        data=body,
        headers={"Authorization":f"Bearer {key}","Content-Type":"application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        raw=resp.read()
        pathlib.Path(out_path).write_bytes(raw)
        data=json.loads(raw)
        return data["choices"][0]["message"]["content"].strip()

g=chat([
    {"role":"system","content":"Fix grammar only. Return only the corrected text. No preamble."},
    {"role":"user","content":"um please add a login button to the settings page"},
], f"/tmp/ds_grammar_$i.json")
print(f"GRAMMAR_OK::{g}")

v=chat([
    {"role":"system","content":"Generate a perfect Vibe Coding prompt. Return ONLY the prompt. Preserve proper nouns. Use [FILL: ...] when unknown."},
    {"role":"user","content":f"Create a perfect Vibe Coding prompt from this corrected speech:\n\n{g}"},
], f"/tmp/ds_vibe_$i.json")
pathlib.Path(f"/tmp/ds_vibe_prompt_$i.txt").write_text(v)
print(f"VIBE_OK::{len(v)}")

ctx=(root/"context/project.md").read_text()+"\n"+(root/"constitutions/vibe-coding.md").read_text()
r=chat([
    {"role":"system","content":"Refine the Vibe Coding prompt using project context. Return ONLY the refined prompt."},
    {"role":"user","content":f"Selected generated prompt:\n\n{v}\n\n---\nProject context:\n\n{ctx}\n\n---\nProduce the refined Vibe Coding prompt."},
], f"/tmp/ds_refine_$i.json")
print(f"REFINE_OK::{len(r)}")
# Placeholder checklist for the user's Test template: FILL markers may remain by design.
print("PLACEHOLDERS_NOTE::FILL markers in prompts are intentional when details are unknown")
PY
  # Parse python status lines
  # shellcheck disable=SC2181
  if [[ $? -eq 0 ]]; then
    ok "DeepSeek grammar+vibe+refine pipeline round $i"
  else
    bad "DeepSeek pipeline round $i"
  fi
done

round "6) Hotkey registration in source (twice-read)"
for i in 1 2; do
  if rg -q 'Modifiers::CONTROL.*, Code::Digit1' "$ROOT/src-tauri/src/lib.rs" \
    && rg -q 'Modifiers::CONTROL.*, Code::Digit2' "$ROOT/src-tauri/src/lib.rs"; then
    ok "Control+1/+2 registered in lib.rs read $i"
  else
    bad "hotkeys missing read $i"
  fi
  if rg -q '"vibe"' "$ROOT/src-tauri/src/dictate.rs" && rg -q '"vibe_refine"' "$ROOT/src-tauri/src/dictate.rs"; then
    ok "vibe modes in dictate.rs read $i"
  else
    bad "vibe modes missing read $i"
  fi
done

round "7) Compile TypeScript + Rust (twice)"
cd "$ROOT"
for i in 1 2; do
  if pnpm exec tsc --noEmit >/tmp/tsc_$i.log 2>&1; then ok "tsc round $i"; else bad "tsc round $i"; cat /tmp/tsc_$i.log; fi
done
cd "$ROOT/src-tauri"
for i in 1 2; do
  if cargo check >/tmp/cargo_$i.log 2>&1; then ok "cargo check round $i"; else bad "cargo check round $i"; tail -30 /tmp/cargo_$i.log; fi
done

round "8) Built app present + process"
APP="$ROOT/src-tauri/target/release/bundle/macos/Flow.app"
if [[ -d "$APP" ]]; then ok "Flow.app bundle exists"; else bad "Flow.app missing"; fi
if pgrep -f "Flow.app/Contents/MacOS" >/dev/null 2>&1 || pgrep -x flow-app >/dev/null 2>&1; then
  ok "Flow process running"
else
  # launch for smoke
  open "$APP" || true
  sleep 2
  if pgrep -f "Flow.app/Contents/MacOS" >/dev/null 2>&1 || pgrep -x flow-app >/dev/null 2>&1; then
    ok "Flow launched for smoke"
  else
    bad "Flow process not running after open"
  fi
fi

round "9) Accessibility / paste prerequisites"
# Flow needs Accessibility; we can only check TCC-ish via osascript probe
if osascript -e 'tell application "System Events" to get name of first process whose frontmost is true' >/tmp/front_app.txt 2>/tmp/front_err.txt; then
  ok "System Events readable (Accessibility likely OK for terminal) front=$(cat /tmp/front_app.txt)"
else
  bad "System Events blocked: $(cat /tmp/front_err.txt)"
fi

round "SUMMARY"
echo "PASS=$PASS FAIL=$FAIL"
if [[ "$FAIL" -gt 0 ]]; then exit 1; fi
echo "ALL DOUBLE-TESTS PASSED"
