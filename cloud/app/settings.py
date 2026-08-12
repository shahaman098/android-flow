from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    # MyGCP
    gcp_project_id: str = "project-ced3b331-e814-4d72-8bc"
    gcp_location: str = "europe-west2"
    stt_model: str = "latest_long"
    default_language: str = "en-GB"

    # Open-weight Qwen2.5-3B on MyGCP flow-llm VM (Ollama + auth proxy)
    # Env names keep HERMES_* for Cloud Run secret wiring compatibility.
    hermes_base_url: str = "http://10.154.0.5:8080"
    # 3b, not 14b: the flow-llm VM (n2-standard-8) has no GPU, and 14b's ~4 tok/s
    # can't finish inside llm.py's 45-60s call budget — it always hits the
    # exception fallback. 3b is the safety default if HERMES_MODEL is ever unset.
    hermes_model: str = "qwen2.5:3b"
    hermes_api_key: str = ""

    # Shared secret: Mac Flow → Cloud Run
    flow_api_key: str = ""


settings = Settings()
