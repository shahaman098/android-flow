use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::json;
use std::process::Command;
use tauri::{AppHandle, Emitter};

use crate::cloud_api::{self, ProcessPayload};
use crate::config::{llm_key_configured, load_config, AppConfig};
use crate::focus::{
    hide_bubble, paste_into_target_app, select_all_and_capture, take_selected_text, target_app_name,
};
use crate::store::{add_history, load_store};
use crate::stt_gcp;
use crate::vibe_context;

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[tauri::command]
pub async fn get_config() -> Result<AppConfig, String> {
    Ok(load_config())
}

#[tauri::command]
pub async fn save_user_config(config: AppConfig) -> Result<(), String> {
    crate::config::save_config(&config)
}

/// Returns remaining setup steps. Empty means processing is configured.
#[tauri::command]
pub async fn get_readiness() -> Result<Vec<String>, String> {
    let cfg = load_config();
    let mut gaps = Vec::new();

    if cloud_api::uses_cloud(&cfg) {
        if cfg.flow_api_url.trim().is_empty() {
            gaps.push(
                "FLOW_API_URL missing. Deploy with cloud/deploy.sh (MyGCP Cloud Run)."
                    .into(),
            );
        }
        if cfg.flow_api_key.trim().is_empty() {
            gaps.push(
                "FLOW_API_KEY missing. Re-run cloud/deploy.sh or pull Secret Manager flow-api-key."
                    .into(),
            );
        }
        return Ok(gaps);
    }

    if cfg.gcp_project_id.trim().is_empty() {
        gaps.push("Set GCP project ID (MyGCP: project-ced3b331-e814-4d72-8bc).".into());
    }
    match stt_gcp::adc_access_token() {
        Ok(_) => {}
        Err(e) => gaps.push(e),
    }
    if !llm_key_configured(&cfg) {
        gaps.push(
            "Add DeepSeek API key in Settings, or DEEPSEEK_API_KEY in Application Support/voice-flow/.env."
                .into(),
        );
    }
    Ok(gaps)
}

#[tauri::command]
pub async fn transcribe_partial(audio_base64: String) -> Result<String, String> {
    let config = load_config();
    ensure_providers(&config)?;

    let audio_bytes = STANDARD
        .decode(audio_base64.trim())
        .map_err(|e| format!("Invalid audio data: {e}"))?;
    if audio_bytes.len() < 2000 {
        return Err(format!(
            "Audio too short for live partial ({} bytes).",
            audio_bytes.len()
        ));
    }

    if cloud_api::uses_cloud(&config) {
        return cloud_api::transcribe(&config, audio_base64.trim()).await;
    }
    stt_gcp::transcribe(&config, audio_bytes).await
}

#[tauri::command]
pub async fn process_dictation(
    app: AppHandle,
    audio_base64: String,
    mode: String,
) -> Result<String, String> {
    let result = process_dictation_inner(&app, audio_base64, mode).await;
    if result.is_err() {
        hide_bubble(&app);
    }
    result
}

#[tauri::command]
pub fn hide_bubble_window(app: AppHandle) -> Result<(), String> {
    hide_bubble(&app);
    Ok(())
}

async fn process_dictation_inner(
    app: &AppHandle,
    audio_base64: String,
    mode: String,
) -> Result<String, String> {
    let config = load_config();
    ensure_providers(&config)?;

    // MyGCP Cloud Run path — STT + LLM entirely on GCP.
    if cloud_api::uses_cloud(&config)
        && matches!(mode.as_str(), "vibe" | "vibe_refine" | "dictate")
    {
        return process_via_cloud(app, &config, audio_base64, &mode).await;
    }

    let audio_bytes = if audio_base64.trim().is_empty() {
        Vec::new()
    } else {
        STANDARD
            .decode(audio_base64.trim())
            .map_err(|e| format!("Invalid audio data: {e}"))?
    };

    let store = load_store();

    let raw_text = if !audio_bytes.is_empty() && audio_bytes.len() < 1500 {
        return Err(format!(
            "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
            audio_bytes.len()
        ));
    } else if audio_bytes.len() >= 1500 {
        let _ = app.emit("dictation-status", "transcribing");
        let text = stt_gcp::transcribe(&config, audio_bytes)
            .await?
            .trim()
            .to_string();
        if text.is_empty() {
            return Err("Speech-to-Text returned empty text (no speech detected).".into());
        }
        text
    } else {
        String::new()
    };

    if !raw_text.is_empty() {
        let _ = app.emit("partial-transcript", &raw_text);
    }

    let app_name = target_app_name(app);
    // Modes: vibe (Control+1), vibe_refine (Control+2). Other modes kept for internal reuse.
    let final_text = match mode.as_str() {
        // Control+1: speech → STT → grammar → perfect Vibe Coding prompt
        "vibe" => {
            if raw_text.is_empty() {
                return Err("No speech detected. Hold fn+1 (or Control+1) and speak to build a Vibe Coding prompt.".into());
            }
            let expanded = expand_snippets(&raw_text, &store.snippets);
            let _ = app.emit("dictation-status", "correcting");
            let corrected =
                polish_text(&config, &expanded, app_name.as_deref(), &store).await?;
            let _ = app.emit("partial-transcript", &corrected);
            let _ = app.emit("dictation-status", "correcting");
            generate_vibe_prompt(&config, &corrected).await?
        }
        // Control+2: select entire prompt + project context → refined Vibe Coding prompt
        "vibe_refine" => {
            let selected = take_selected_text(app).ok_or_else(|| {
                "No prompt text captured. Focus the editor holding your generated prompt, then press fn+2 (or Control+2).".to_string()
            })?;
            let _ = app.emit("partial-transcript", &selected);
            let _ = app.emit("dictation-status", "correcting");
            refine_vibe_prompt(&config, &selected).await?
        }
        "command" => {
            if raw_text.is_empty() {
                return Err("No speech detected for command mode.".into());
            }
            let selected = take_selected_text(app)
                .ok_or_else(|| "No selected text for command mode.".to_string())?;
            let _ = app.emit("dictation-status", "correcting");
            run_command(&config, &selected, &raw_text, app_name.as_deref()).await?
        }
        "prompt" => {
            let selected = take_selected_text(app);
            let source = build_prompt_source(selected.as_deref(), &raw_text);
            if source.trim().is_empty() {
                return Err(
                    "Prompt mode needs selected text and/or speech to optimize.".into(),
                );
            }
            let _ = app.emit("partial-transcript", &source);
            let _ = app.emit("dictation-status", "correcting");
            optimize_to_prompt(&config, &source, app_name.as_deref()).await?
        }
        _ => {
            if raw_text.is_empty() {
                return Err("No speech detected.".into());
            }
            let expanded = expand_snippets(&raw_text, &store.snippets);
            let _ = app.emit("dictation-status", "correcting");
            polish_text(&config, &expanded, app_name.as_deref(), &store).await?
        }
    };

    let final_text = final_text.trim().to_string();
    if final_text.is_empty() {
        return Err("Model returned empty text.".into());
    }

    let _ = app.emit("dictation-status", "pasting");
    paste_into_target_app(app, &final_text)?;
    if let Err(e) = add_history(&final_text, app_name, &mode) {
        let _ = app.emit(
            "dictation-warning",
            format!("Pasted successfully, but history save failed: {e}"),
        );
    }
    hide_bubble(app);

    let _ = app.emit("dictation-done", &final_text);
    Ok(final_text)
}

fn ensure_providers(config: &AppConfig) -> Result<(), String> {
    if cloud_api::uses_cloud(config) {
        if config.flow_api_url.trim().is_empty() || config.flow_api_key.trim().is_empty() {
            return Err(
                "Cloud processing requires FLOW_API_URL and FLOW_API_KEY (run cloud/deploy.sh)."
                    .into(),
            );
        }
        return Ok(());
    }
    if config.stt_provider != "gcp_speech" {
        return Err(format!(
            "STT provider must be gcp_speech (got '{}'). No OpenAI/Whisper fallback.",
            config.stt_provider
        ));
    }
    if config.gcp_project_id.trim().is_empty() {
        return Err("GCP project ID is missing.".into());
    }
    if !llm_key_configured(config) {
        return Err(
            "DeepSeek API key missing. Add it in Settings or DEEPSEEK_API_KEY in voice-flow/.env."
                .into(),
        );
    }
    if config.llm_provider != "deepseek" {
        return Err(format!(
            "LLM provider must be deepseek (got '{}'). No OpenAI fallback.",
            config.llm_provider
        ));
    }
    Ok(())
}

async fn process_via_cloud(
    app: &AppHandle,
    config: &AppConfig,
    audio_base64: String,
    mode: &str,
) -> Result<String, String> {
    let constitution = vibe_context::load_constitution();
    let project_context = vibe_context::load_project_context().unwrap_or_default();
    let skill_id = if mode == "vibe_refine" {
        "refine-prompt"
    } else if mode == "dictate" {
        "grammar-correct"
    } else {
        "vibe-prompt"
    };
    let skill = vibe_context::load_skill_blurb(skill_id);
    let store = load_store();
    let dictionary: Vec<String> = store.dictionary.iter().map(|d| d.word.clone()).collect();

    let (selected_text, audio) = if mode == "vibe_refine" {
        let selected = take_selected_text(app).ok_or_else(|| {
            "No prompt text captured. Focus the editor holding your generated prompt, then press fn+2 (or Control+2).".to_string()
        })?;
        let _ = app.emit("partial-transcript", &selected);
        let _ = app.emit("dictation-status", "correcting");
        (Some(selected), None)
    } else {
        if audio_base64.trim().is_empty() {
            return Err("No speech detected. Hold fn+1 (or Control+1) and speak to build a Vibe Coding prompt.".into());
        }
        let bytes = STANDARD
            .decode(audio_base64.trim())
            .map_err(|e| format!("Invalid audio data: {e}"))?;
        if bytes.len() < 1500 {
            return Err(format!(
                "Audio too short ({} bytes). Hold the hotkey longer while speaking.",
                bytes.len()
            ));
        }
        let _ = app.emit("dictation-status", "transcribing");
        (None, Some(audio_base64.trim().to_string()))
    };

    let _ = app.emit("dictation-status", "correcting");
    let payload = ProcessPayload {
        mode,
        audio_base64: audio.as_deref(),
        selected_text: selected_text.as_deref(),
        project_context: Some(project_context.as_str()),
        constitution: Some(constitution.as_str()),
        skill: Some(skill.as_str()),
        language: Some(config.language.as_str()),
        dictionary,
    };

    let (final_text, raw) = cloud_api::process(config, payload).await?;
    if let Some(raw) = raw {
        if !raw.trim().is_empty() {
            let _ = app.emit("partial-transcript", raw.trim());
        }
    }

    let final_text = final_text.trim().to_string();
    if final_text.is_empty() {
        return Err("MyGCP Cloud Run returned empty text.".into());
    }

    let app_name = target_app_name(app);
    let _ = app.emit("dictation-status", "pasting");
    paste_into_target_app(app, &final_text)?;
    if let Err(e) = add_history(&final_text, app_name, mode) {
        let _ = app.emit(
            "dictation-warning",
            format!("Pasted successfully, but history save failed: {e}"),
        );
    }
    hide_bubble(app);
    let _ = app.emit("dictation-done", &final_text);
    Ok(final_text)
}

#[tauri::command]
pub async fn verify_stt_connection() -> Result<String, String> {
    let config = load_config();
    if cloud_api::uses_cloud(&config) {
        return cloud_api::health(&config).await;
    }
    stt_gcp::verify_speech(&config).await
}

#[tauri::command]
pub async fn verify_llm_connection() -> Result<String, String> {
    let config = load_config();
    if cloud_api::uses_cloud(&config) {
        return cloud_api::health(&config).await;
    }
    if !llm_key_configured(&config) {
        return Err("No DeepSeek API key configured.".into());
    }

    let base = config.llm_base_url.trim().trim_end_matches('/');
    let client = reqwest::Client::new();
    let body = json!({
        "model": config.llm_model,
        "temperature": 0.0,
        "thinking": { "type": "disabled" },
        "messages": [
            { "role": "user", "content": "Reply with exactly: ok" }
        ]
    });

    let response = client
        .post(format!("{base}/chat/completions"))
        .bearer_auth(config.llm_api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("DeepSeek network error: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("DeepSeek auth/API error ({status}): {body}"));
    }

    Ok(format!(
        "DeepSeek OK — model {} at {}.",
        config.llm_model, base
    ))
}

/// Pull DeepSeek key from MyGCP Secret Manager into local config + .env (never committed).
#[tauri::command]
pub async fn import_deepseek_from_gcloud() -> Result<String, String> {
    let mut config = load_config();
    let project = if config.gcp_project_id.trim().is_empty() {
        "project-ced3b331-e814-4d72-8bc".to_string()
    } else {
        config.gcp_project_id.clone()
    };

    let output = Command::new("gcloud")
        .args([
            "secrets",
            "versions",
            "access",
            "latest",
            "--secret=n8n-deepseek-api-key",
            &format!("--project={project}"),
        ])
        .output()
        .map_err(|e| format!("Could not run gcloud: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Failed to read Secret Manager n8n-deepseek-api-key: {stderr}"));
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() || !key.starts_with("sk-") {
        return Err("Secret Manager returned an empty or unexpected DeepSeek key.".into());
    }

    config.llm_api_key = key.clone();
    config.llm_provider = "deepseek".into();
    crate::config::save_config(&config)?;

    let dir = crate::config::data_dir()?;
    let env_path = dir.join(".env");
    let mut env_body = String::new();
    if env_path.exists() {
        if let Ok(existing) = std::fs::read_to_string(&env_path) {
            for line in existing.lines() {
                if line.trim().starts_with("DEEPSEEK_API_KEY=") {
                    continue;
                }
                env_body.push_str(line);
                env_body.push('\n');
            }
        }
    }
    env_body.push_str(&format!("DEEPSEEK_API_KEY={key}\n"));
    std::fs::write(&env_path, env_body).map_err(|e| e.to_string())?;

    Ok("Imported DeepSeek key from Secret Manager into local config and voice-flow/.env.".into())
}

/// Control+2 entry point: select-all → load context → refine → paste (no speech).
pub async fn process_vibe_refine(app: AppHandle) -> Result<String, String> {
    let _ = app.emit("dictation-status", "correcting");
    let result = process_dictation_inner(&app, String::new(), "vibe_refine".into()).await;
    if result.is_err() {
        hide_bubble(&app);
    }
    result
}

/// Capture frontmost app + select-all prompt text for Control+2.
pub fn prepare_vibe_refine(app: &AppHandle) -> Result<String, String> {
    crate::focus::capture_frontmost_app(app)?;
    select_all_and_capture(app)
}

/// Control+1 skill: grammar-corrected idea → perfect Vibe Coding prompt.
async fn generate_vibe_prompt(config: &AppConfig, corrected: &str) -> Result<String, String> {
    let constitution = vibe_context::load_constitution();
    let skill = vibe_context::load_skill_blurb("vibe-prompt");

    let system = format!(
        r#"You generate a perfect Vibe Coding prompt from the user's grammar-corrected speech.

You are NOT implementing the task — you only write the prompt text an AI coding agent will receive.

Constitution:
{constitution}

Skill notes:
{skill}

Rules:
- Preserve all proper nouns exactly
- Follow the canonical template from the constitution exactly: all ten sections, in order, none empty
- Use [FILL: …] placeholders where specifics are unknown
- Do not invent features beyond the user's request
- Return ONLY the prompt — no preamble or quotes
- Language preference code: {lang}"#,
        constitution = constitution,
        skill = skill,
        lang = config.language,
    );

    let user = format!(
        "Create a perfect Vibe Coding prompt from this corrected speech:\n\n{corrected}"
    );
    chat_completion(config, &system, &user).await
}

/// Control+2 skill: selected prompt + context/ → refined Vibe Coding prompt.
async fn refine_vibe_prompt(config: &AppConfig, selected_prompt: &str) -> Result<String, String> {
    let constitution = vibe_context::load_constitution();
    let project_context = vibe_context::load_project_context()?;
    let skill = vibe_context::load_skill_blurb("refine-prompt");

    let system = format!(
        r#"You refine an existing Vibe Coding prompt using the project's context files.

You are NOT implementing the task — you only improve the prompt.

Constitution:
{constitution}

Skill notes:
{skill}

Rules:
- Keep the user's core intent from the selected prompt
- Weave in relevant facts from project context
- Preserve every canonical template section from the selected prompt
- Preserve proper nouns exactly
- Prefer [FILL: …] when details are still missing
- Do not introduce features beyond those listed in the selected prompt + context
- Return ONLY the refined prompt — no preamble or quotes
- Language preference code: {lang}"#,
        constitution = constitution,
        skill = skill,
        lang = config.language,
    );

    let user = format!(
        "Selected generated prompt:\n\n{selected_prompt}\n\n---\nProject context:\n\n{project_context}\n\n---\nProduce the refined Vibe Coding prompt."
    );
    chat_completion(config, &system, &user).await
}

fn build_prompt_source(selected: Option<&str>, spoken: &str) -> String {
    let selected = selected.map(str::trim).filter(|s| !s.is_empty());
    let spoken = spoken.trim();
    match (selected, spoken.is_empty()) {
        (Some(sel), true) => sel.to_string(),
        (None, false) => spoken.to_string(),
        (Some(sel), false) => format!(
            "Rough draft / selected text:\n{sel}\n\nSpoken intent / refinements:\n{spoken}"
        ),
        (None, true) => String::new(),
    }
}

async fn optimize_to_prompt(
    config: &AppConfig,
    rough: &str,
    app_name: Option<&str>,
) -> Result<String, String> {
    let app_hint = app_name.unwrap_or("unknown app");
    let system = format!(
        r#"You are an expert prompt engineer (similar to prompt-optimizer).

Task: Rewrite the user's rough idea into a precise, high-quality prompt for an AI model.
You are NOT answering or executing their request — you only improve the prompt text.

Rules:
1. Keep the user's core intent
2. Make the prompt specific, actionable, and well-structured
3. Add clear role, goal, constraints, and output format when useful
4. Remove filler and vagueness
5. Prefer English unless the rough text is clearly another language
6. Preserve any {{{{variable}}}} placeholders exactly
7. Return ONLY the optimized prompt — no preamble, quotes, or explanation
8. Target app context (optional tone): {app_hint}
9. Language preference code: {lang}"#,
        app_hint = app_hint,
        lang = config.language,
    );

    let user = format!(
        "Optimize this rough prompt. Output only the improved prompt text.\n\n{}",
        rough
    );

    chat_completion(config, &system, &user).await
}

fn expand_snippets(text: &str, snippets: &[crate::store::Snippet]) -> String {
    let mut out = text.to_string();
    let mut sorted = snippets.to_vec();
    sorted.sort_by(|a, b| b.trigger.len().cmp(&a.trigger.len()));
    for snippet in sorted {
        let trigger = snippet.trigger.trim();
        if trigger.is_empty() {
            continue;
        }
        if out.eq_ignore_ascii_case(trigger)
            || out
                .to_lowercase()
                .contains(&format!("snippet {}", trigger.to_lowercase()))
        {
            return snippet.expansion.clone();
        }
        if let Some(idx) = find_case_insensitive(&out, trigger) {
            let mut replaced = String::new();
            replaced.push_str(&out[..idx]);
            replaced.push_str(&snippet.expansion);
            replaced.push_str(&out[idx + trigger.len()..]);
            out = replaced;
        }
    }
    out
}

/// Byte offset of `needle` in `haystack`, ignoring ASCII case.
///
/// Searching a `to_lowercase()` copy and then slicing the original is not safe: lowercasing
/// can change a string's byte length (e.g. `İ` is 2 bytes, its lowercase form is 3), so the
/// index drifts and the slice either cuts the wrong span or panics on a char boundary.
/// Dictation text is arbitrary user speech, so that path was reachable.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let hay = haystack.as_bytes();
    let pat = needle.as_bytes();
    (0..=hay.len() - pat.len())
        // Only offsets that start a character can be spliced back into the original string.
        .filter(|&i| haystack.is_char_boundary(i) && haystack.is_char_boundary(i + pat.len()))
        .find(|&i| hay[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

async fn polish_text(
    config: &AppConfig,
    text: &str,
    app_name: Option<&str>,
    store: &crate::store::FlowStore,
) -> Result<String, String> {
    let style = store
        .styles
        .iter()
        .find(|s| s.id == config.active_style_id)
        .map(|s| s.prompt.as_str())
        .unwrap_or("Write clear, natural prose.");

    let app_tone = if config.app_aware_tone {
        app_aware_instruction(app_name)
    } else {
        String::new()
    };

    let dict = store
        .dictionary
        .iter()
        .map(|d| d.word.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let system = format!(
        r#"You clean up voice dictation into written text.

Rules:
- Fix grammar, spelling, punctuation, and capitalization
- Remove filler words (um, uh, like, you know) unless meaningful
- Keep the speaker's meaning
- Do not add greetings, explanations, or quotes
- Return only the corrected text
- Language preference code: {lang}
- Style: {style}
- App tone guidance: {app_tone}
- Preserve these dictionary terms exactly when present: {dict}"#,
        lang = config.language,
        style = style,
        app_tone = if app_tone.is_empty() {
            "none"
        } else {
            &app_tone
        },
        dict = if dict.is_empty() { "none" } else { &dict },
    );

    chat_completion(config, &system, text).await
}

async fn run_command(
    config: &AppConfig,
    selected: &str,
    instruction: &str,
    app_name: Option<&str>,
) -> Result<String, String> {
    let app_tone = if config.app_aware_tone {
        app_aware_instruction(app_name)
    } else {
        String::new()
    };

    let system = format!(
        r#"You edit the user's selected text according to their spoken instruction.

Rules:
- Apply only the instruction
- Return only the edited text
- Keep meaning unless asked to change it
- Language preference code: {lang}
- App tone guidance: {app_tone}"#,
        lang = config.language,
        app_tone = if app_tone.is_empty() {
            "none"
        } else {
            &app_tone
        },
    );

    let user = format!("Selected text:\n{selected}\n\nInstruction:\n{instruction}");
    chat_completion(config, &system, &user).await
}

fn app_aware_instruction(app_name: Option<&str>) -> String {
    let Some(name) = app_name.map(|s| s.to_lowercase()) else {
        return String::new();
    };

    if name.contains("slack") || name.contains("discord") || name.contains("messages") {
        "Match casual chat tone; keep it short.".into()
    } else if name.contains("mail") || name.contains("outlook") || name.contains("gmail") {
        "Match professional email tone.".into()
    } else if name.contains("cursor")
        || name.contains("code")
        || name.contains("terminal")
        || name.contains("iterm")
    {
        "Prefer precise technical wording suitable for coding tools.".into()
    } else if name.contains("notion") || name.contains("docs") || name.contains("word") {
        "Match clear document writing.".into()
    } else if name.contains("twitter") || name.contains("x") {
        "Keep it punchy and brief.".into()
    } else {
        format!("Adapt tone to the app: {name}.")
    }
}

async fn chat_completion(config: &AppConfig, system: &str, user: &str) -> Result<String, String> {
    let base = config.llm_base_url.trim().trim_end_matches('/');
    let model = if config.llm_model.trim().is_empty() {
        "deepseek-v4-flash"
    } else {
        config.llm_model.trim()
    };

    let client = reqwest::Client::new();
    let body = json!({
        "model": model,
        "temperature": 0.2,
        "thinking": { "type": "disabled" },
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });

    let response = client
        .post(format!("{base}/chat/completions"))
        .bearer_auth(config.llm_api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("DeepSeek request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("DeepSeek error ({status}): {body}"));
    }

    let parsed: ChatResponse = response
        .json()
        .await
        .map_err(|e| format!("DeepSeek parse error: {e}"))?;

    let content = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default()
        .trim()
        .to_string();

    if content.is_empty() {
        return Err("DeepSeek returned empty content.".into());
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Snippet;

    fn snippet(trigger: &str, expansion: &str) -> Snippet {
        Snippet {
            id: "test".into(),
            trigger: trigger.into(),
            expansion: expansion.into(),
        }
    }

    #[test]
    fn finds_trigger_ignoring_case() {
        assert_eq!(find_case_insensitive("say Hello there", "hello"), Some(4));
        assert_eq!(find_case_insensitive("nothing here", "absent"), None);
        assert_eq!(find_case_insensitive("short", "much longer"), None);
        assert_eq!(find_case_insensitive("anything", ""), None);
    }

    #[test]
    fn expands_trigger_inside_multibyte_speech() {
        // The old implementation searched a lowercased copy and sliced the original at
        // that index. `İ` is 2 bytes but lowercases to 3, so the offset drifted past a
        // char boundary and this input panicked.
        let out = expand_snippets("İstanbul sig now", &[snippet("sig", "Best, Aman")]);
        assert_eq!(out, "İstanbul Best, Aman now");
    }

    #[test]
    fn never_splits_a_multibyte_character() {
        // "é" must not be matched from its trailing byte.
        assert_eq!(find_case_insensitive("café", "é"), Some(3));
        let out = expand_snippets("café", &[snippet("é", "e")]);
        assert_eq!(out, "cafe");
    }

    #[test]
    fn leaves_text_untouched_without_a_match() {
        let out = expand_snippets("plain dictation", &[snippet("sig", "Best, Aman")]);
        assert_eq!(out, "plain dictation");
    }

    #[test]
    fn whole_utterance_matching_the_trigger_becomes_the_expansion() {
        let out = expand_snippets("SIG", &[snippet("sig", "Best, Aman")]);
        assert_eq!(out, "Best, Aman");
    }
}
