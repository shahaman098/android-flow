export type AppConfig = {
  stt_provider: string;
  gcp_project_id: string;
  gcp_location: string;
  stt_model: string;
  llm_provider: string;
  llm_base_url: string;
  llm_model: string;
  llm_api_key: string;
  openai_api_key: string;
  hotkey: string;
  command_hotkey: string;
  prompt_hotkey: string;
  correct_english: boolean;
  whisper_model: string;
  correction_model: string;
  hands_free: boolean;
  /** Soft start/stop cues when recording begins and ends. */
  interaction_sounds: boolean;
  language: string;
  active_style_id: string;
  app_aware_tone: boolean;
  vibe_project_root: string;
  processing_mode: string;
  flow_api_url: string;
  flow_api_key: string;
};

export type DictionaryEntry = { id: string; word: string };
export type Snippet = { id: string; trigger: string; expansion: string };
export type Style = { id: string; name: string; prompt: string };
export type HistoryItem = {
  id: string;
  text: string;
  app_name: string | null;
  created_at: string;
  word_count: number;
  mode: string;
};
export type Stats = {
  total_dictations: number;
  total_words: number;
  total_commands: number;
  total_prompts: number;
};
export type TranscriptSegment = {
  speaker: "you" | "other";
  text: string;
  at_ms: number;
};
export type MeetingTranscript = {
  id: string;
  app_name: string;
  started_at: string;
  ended_at: string;
  segments: TranscriptSegment[];
};
export type FlowStore = {
  dictionary: DictionaryEntry[];
  snippets: Snippet[];
  styles: Style[];
  history: HistoryItem[];
  stats: Stats;
  meetings: MeetingTranscript[];
};

export type MeetingPhase = "idle" | "detected" | "notified" | "capturing";
export type MeetingStatus = {
  phase: MeetingPhase;
  app_name: string | null;
  started_at: string | null;
};

export type AccessibilityStatus = {
  trusted: boolean;
  executable_path: string;
  app_bundle_path: string | null;
  guidance: string | null;
};

export type Status =
  | "idle"
  | "recording"
  | "transcribing"
  | "correcting"
  | "pasting"
  | "error";
