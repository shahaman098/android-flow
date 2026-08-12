use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

const DEFAULT_GCP_PROJECT: &str = "project-ced3b331-e814-4d72-8bc";
const DEFAULT_GCP_LOCATION: &str = "europe-west2";
const DEFAULT_LLM_BASE: &str = "https://api.deepseek.com";
const DEFAULT_LLM_MODEL: &str = "deepseek-v4-flash";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub stt_provider: String,
    pub gcp_project_id: String,
    pub gcp_location: String,
    pub stt_model: String,
    pub llm_provider: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub llm_api_key: String,
    /// Legacy field kept for migration only; unused when DeepSeek is configured.
    #[serde(default)]
    pub openai_api_key: String,
    pub hotkey: String,
    pub command_hotkey: String,
    pub prompt_hotkey: String,
    pub correct_english: bool,
    pub whisper_model: String,
    pub correction_model: String,
    pub hands_free: bool,
    /// Soft start/stop cues when recording begins and ends.
    pub interaction_sounds: bool,
    pub language: String,
    pub active_style_id: String,
    pub app_aware_tone: bool,
    /// Absolute path to the Vibe Coding project root (contains context/, skills/, constitutions/).
    pub vibe_project_root: String,
    /// `cloud` = MyGCP Cloud Run processing; `local` = Mac calls Speech/LLM directly.
    pub processing_mode: String,
    pub flow_api_url: String,
    pub flow_api_key: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            stt_provider: "gcp_speech".into(),
            gcp_project_id: DEFAULT_GCP_PROJECT.into(),
            gcp_location: DEFAULT_GCP_LOCATION.into(),
            // europe-west2 supports latest_long + en-GB; latest_short rejects en-US there.
            stt_model: "latest_long".into(),
            llm_provider: "deepseek".into(),
            llm_base_url: DEFAULT_LLM_BASE.into(),
            llm_model: DEFAULT_LLM_MODEL.into(),
            llm_api_key: resolve_env_key("DEEPSEEK_API_KEY"),
            openai_api_key: String::new(),
            // Vibe Coding shortcuts (do not change intended meaning).
            hotkey: "Section".into(),
            prompt_hotkey: "Section+1".into(),
            command_hotkey: "".into(),
            correct_english: true,
            whisper_model: "latest_long".into(),
            correction_model: DEFAULT_LLM_MODEL.into(),
            // Tap Control+1 to start, tap again to stop — more reliable than hold on macOS.
            hands_free: true,
            interaction_sounds: true,
            language: "en".into(),
            active_style_id: "clear".into(),
            app_aware_tone: true,
            vibe_project_root: String::new(),
            processing_mode: {
                let mode = resolve_env_key("PROCESSING_MODE");
                if mode.is_empty() {
                    "cloud".into()
                } else {
                    mode
                }
            },
            flow_api_url: resolve_env_key("FLOW_API_URL"),
            flow_api_key: resolve_env_key("FLOW_API_KEY"),
        }
    }
}

fn resolve_env_key(name: &str) -> String {
    if let Ok(key) = std::env::var(name) {
        if !key.trim().is_empty() {
            return key.trim().to_string();
        }
    }
    if let Ok(dir) = data_dir() {
        if let Ok(raw) = fs::read_to_string(dir.join(".env")) {
            for line in raw.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let prefix = format!("{name}=");
                if let Some(value) = line.strip_prefix(&prefix) {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        return value.to_string();
                    }
                }
            }
        }
    }
    String::new()
}

pub fn data_dir() -> Result<PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "Could not resolve config directory".to_string())?
        .join("voice-flow");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn config_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("config.json"))
}

pub fn load_config() -> AppConfig {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return AppConfig::default(),
    };

    if !path.exists() {
        let cfg = AppConfig::default();
        let _ = save_config(&cfg);
        return cfg;
    }

    let mut cfg = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    };
    let mut dirty = false;

    if cfg.stt_provider.trim().is_empty() {
        cfg.stt_provider = "gcp_speech".into();
        dirty = true;
    }
    if cfg.gcp_project_id.trim().is_empty() {
        cfg.gcp_project_id = DEFAULT_GCP_PROJECT.into();
        dirty = true;
    }
    if cfg.gcp_location.trim().is_empty() {
        cfg.gcp_location = DEFAULT_GCP_LOCATION.into();
        dirty = true;
    }
    if cfg.stt_model.trim().is_empty() || cfg.stt_model == "latest_short" {
        // Migrate away from latest_short+en-US which fails in europe-west2.
        if cfg.gcp_location == "europe-west2" || cfg.stt_model.trim().is_empty() {
            cfg.stt_model = "latest_long".into();
            dirty = true;
        }
    }
    if cfg.llm_provider.trim().is_empty() {
        cfg.llm_provider = "deepseek".into();
        dirty = true;
    }
    if cfg.llm_base_url.trim().is_empty() {
        cfg.llm_base_url = DEFAULT_LLM_BASE.into();
        dirty = true;
    }
    if cfg.llm_model.trim().is_empty()
        || cfg.llm_model == "gpt-4o-mini"
        || cfg.correction_model == "gpt-4o-mini"
    {
        cfg.llm_model = DEFAULT_LLM_MODEL.into();
        cfg.correction_model = DEFAULT_LLM_MODEL.into();
        dirty = true;
    }
    // Prefer env/.env when config file has no DeepSeek key yet.
    if cfg.llm_api_key.trim().is_empty() {
        let from_env = resolve_env_key("DEEPSEEK_API_KEY");
        if !from_env.is_empty() {
            cfg.llm_api_key = from_env;
            dirty = true;
        }
    }
    // Env / .env is a *fallback* that fills blanks, never an override. It used to win on
    // every load, which silently reverted anything saved from Hub Settings — picking
    // "local" mode or pasting a different Cloud Run URL appeared to work, then undid
    // itself on the next read.
    if cfg.processing_mode.trim().is_empty() {
        let mode = resolve_env_key("PROCESSING_MODE");
        cfg.processing_mode = if mode.is_empty() { "cloud".into() } else { mode };
        dirty = true;
    }
    if cfg.flow_api_url.trim().is_empty() {
        let url = resolve_env_key("FLOW_API_URL");
        if !url.is_empty() {
            cfg.flow_api_url = url;
            dirty = true;
        }
    }
    if cfg.flow_api_key.trim().is_empty() {
        let key = resolve_env_key("FLOW_API_KEY");
        if !key.is_empty() {
            cfg.flow_api_key = key;
            dirty = true;
        }
    }
    if !cfg.flow_api_url.trim().is_empty()
        && !cfg.flow_api_key.trim().is_empty()
        && cfg.llm_api_key.trim().is_empty()
        && !cfg.processing_mode.trim().eq_ignore_ascii_case("cloud")
    {
        // A configured Cloud Run endpoint means the Mac should not ask for direct
        // DeepSeek credentials. Prefer the managed cloud path unless local mode
        // has its own LLM key configured.
        cfg.processing_mode = "cloud".into();
        dirty = true;
    }
    // One-time migration: old OpenAI-only configs.
    if cfg.llm_api_key.trim().is_empty() && !cfg.openai_api_key.trim().is_empty() {
        // Keep openai key in file but do not use it for the MyGCP pipeline.
    }

    // Migrate to Vibe Coding Control+1 / Control+2.
    if matches!(
        cfg.hotkey.as_str(),
        "Alt+Space" | "Option+Space" | "Command+1" | ""
    ) {
        cfg.hotkey = "Control+1".into();
        dirty = true;
    }
    if matches!(
        cfg.prompt_hotkey.as_str(),
        "Control+Alt+Space" | "Ctrl+Alt+Space" | "Command+2" | ""
    ) {
        cfg.prompt_hotkey = "Control+2".into();
        dirty = true;
    }
    if !cfg.correct_english {
        cfg.correct_english = true;
        dirty = true;
    }

    if dirty {
        let _ = save_config(&cfg);
    }

    cfg
}

pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let path = config_path()?;
    let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    file.write_all(raw.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn llm_key_configured(config: &AppConfig) -> bool {
    !config.llm_api_key.trim().is_empty()
}

pub fn language_bcp47(language: &str) -> String {
    match language.trim() {
        // Default English → en-GB: required for Speech latest_* models in europe-west2.
        "" | "auto" | "en" => "en-GB".into(),
        "en-us" | "en_US" | "en-US" => "en-US".into(),
        "en-gb" | "en_GB" | "en-GB" => "en-GB".into(),
        other if other.contains('-') => other.to_string(),
        other => format!("{other}-GB"),
    }
}
