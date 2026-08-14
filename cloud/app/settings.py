from pydantic import AliasChoices, Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    # Speech. Groq is the default cheapest cloud path; GCP Speech remains as a
    # fallback when no Groq key is available or accuracy is preferred.
    stt_provider: str = Field(
        default="groq_whisper",
        validation_alias=AliasChoices("STT_PROVIDER", "stt_provider"),
    )
    groq_api_key: str = Field(
        default="",
        validation_alias=AliasChoices("GROQ_API_KEY", "groq_api_key"),
    )
    groq_stt_model: str = Field(
        default="whisper-large-v3-turbo",
        validation_alias=AliasChoices("GROQ_STT_MODEL", "groq_stt_model"),
    )
    gcp_project_id: str = "project-ced3b331-e814-4d72-8bc"
    gcp_location: str = "europe-west2"
    stt_model: str = "latest_long"
    default_language: str = "en-GB"

    # OpenAI-compatible LLM. Env names keep HERMES_* compatibility with the old
    # Ollama VM path, while DEEPSEEK_* is the cheap serverless default.
    llm_provider: str = Field(
        default="deepseek",
        validation_alias=AliasChoices("LLM_PROVIDER", "HERMES_PROVIDER", "llm_provider"),
    )
    hermes_base_url: str = Field(
        default="https://api.deepseek.com",
        validation_alias=AliasChoices("HERMES_BASE_URL", "DEEPSEEK_BASE_URL", "XAI_BASE_URL", "LLM_BASE_URL"),
    )
    hermes_model: str = Field(
        default="deepseek-v4-flash",
        validation_alias=AliasChoices("HERMES_MODEL", "DEEPSEEK_MODEL", "XAI_MODEL", "LLM_MODEL"),
    )
    hermes_api_key: str = Field(
        default="",
        validation_alias=AliasChoices("HERMES_API_KEY", "DEEPSEEK_API_KEY", "XAI_API_KEY", "LLM_API_KEY"),
    )

    # Shared secret: Mac Flow → Cloud Run
    flow_api_key: str = ""


settings = Settings()
