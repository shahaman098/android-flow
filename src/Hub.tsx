import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AccessibilityStatus,
  AppConfig,
  FlowStore,
  MeetingStatus,
  TrainingStats,
} from "./lib/types";
import { useMeetingCapture } from "./lib/useMeetingCapture";

type Tab =
  | "overview"
  | "history"
  | "meetings"
  | "dictionary"
  | "snippets"
  | "styles"
  | "settings";

const LANGS = [
  { id: "en", label: "English" },
  { id: "es", label: "Spanish" },
  { id: "fr", label: "French" },
  { id: "de", label: "German" },
  { id: "pt", label: "Portuguese" },
  { id: "it", label: "Italian" },
  { id: "auto", label: "Auto-detect" },
];

function llmDefaults(provider: string) {
  if (provider === "xai") {
    return {
      llm_base_url: "https://api.x.ai/v1",
      llm_model: "grok-4.3",
      correction_model: "grok-4.3",
    };
  }
  return {
    llm_base_url: "https://api.deepseek.com",
    llm_model: "deepseek-v4-flash",
    correction_model: "deepseek-v4-flash",
  };
}

export default function Hub() {
  const [tab, setTab] = useState<Tab>("overview");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [store, setStore] = useState<FlowStore | null>(null);
  const [error, setError] = useState("");
  const [warning, setWarning] = useState("");
  const [verifyMsg, setVerifyMsg] = useState("");
  const [editingCloud, setEditingCloud] = useState(false);
  const [readiness, setReadiness] = useState<string[]>([]);
  const [word, setWord] = useState("");
  const [trigger, setTrigger] = useState("");
  const [expansion, setExpansion] = useState("");
  const [liveStatus, setLiveStatus] = useState("Idle");
  const didInitialRoute = useRef(false);
  const warningTimer = useRef<number | null>(null);
  const [axTrusted, setAxTrusted] = useState<boolean | null>(null);
  const [axStatus, setAxStatus] = useState<AccessibilityStatus | null>(null);
  const [meetingStatus, setMeetingStatus] = useState<MeetingStatus>({
    phase: "idle",
    app_name: null,
    started_at: null,
  });
  const [meetingError, setMeetingError] = useState("");
  const [screenRecordingTrusted, setScreenRecordingTrusted] = useState<boolean | null>(null);
  const [trainingStats, setTrainingStats] = useState<TrainingStats | null>(null);

  // Owns the mic-chunk restart loop for meeting transcription, reactive to backend phase.
  useMeetingCapture();

  async function refresh() {
    const [cfg, st, ready, training] = await Promise.all([
      invoke<AppConfig>("get_config"),
      invoke<FlowStore>("get_store"),
      invoke<string[]>("get_readiness"),
      invoke<TrainingStats>("get_training_stats").catch(() => null),
    ]);
    setConfig(cfg);
    setStore(st);
    setReadiness(ready);
    setTrainingStats(training);
    // Route to Settings at most once, and only when setup is genuinely incomplete.
    // This previously ran on every refresh — including the one fired by dictation-done —
    // and keyed off llm_api_key, which is empty by design in cloud mode. The result was
    // that the Hub snapped back to Settings on launch and after every single dictation.
    if (!didInitialRoute.current) {
      didInitialRoute.current = true;
      if (ready.length > 0) setTab("settings");
    }
  }

  useEffect(() => {
    refresh().catch((err) => setError(String(err)));
    const pollAx = () => {
      void invoke<AccessibilityStatus>("get_accessibility_status")
        .then((status) => {
          setAxStatus(status);
          setAxTrusted(status.trusted);
        })
        .catch(() => setAxTrusted(false));
    };
    pollAx();
    const axTimer = window.setInterval(pollAx, 2000);

    const pollMeeting = () => {
      void invoke<MeetingStatus>("meeting_get_status")
        .then(setMeetingStatus)
        .catch(() => undefined);
      void invoke<boolean>("check_screen_recording_trusted")
        .then(setScreenRecordingTrusted)
        .catch(() => setScreenRecordingTrusted(false));
    };
    pollMeeting();
    const meetingTimer = window.setInterval(pollMeeting, 3000);

    const unsubs: Array<() => void> = [];
    (async () => {
      unsubs.push(
        await listen("dictation-done", () => {
          void refresh();
          setError("");
          setLiveStatus("Done");
        }),
      );
      unsubs.push(
        await listen<string>("dictation-error", (event) => {
          setError(event.payload);
          setWarning("");
          setLiveStatus("Error");
        }),
      );
      unsubs.push(
        await listen<string>("dictation-warning", (event) => {
          setWarning(event.payload);
          setLiveStatus("Idle");
          if (warningTimer.current != null) {
            window.clearTimeout(warningTimer.current);
          }
          warningTimer.current = window.setTimeout(() => {
            setWarning("");
            warningTimer.current = null;
          }, 4500);
        }),
      );
      unsubs.push(
        await listen<string>("dictation-status", (event) => {
          setLiveStatus(event.payload);
        }),
      );
      unsubs.push(
        await listen<string>("engine-status", (event) => {
          setLiveStatus(event.payload);
        }),
      );
      unsubs.push(
        await listen("recording-start", () => {
          setLiveStatus("Hotkey: recording…");
        }),
      );
      unsubs.push(
        await listen("recording-stop", () => {
          setLiveStatus("Hotkey: processing…");
        }),
      );
      unsubs.push(
        await listen("recording-cancel", () => {
          setLiveStatus("Idle");
        }),
      );
      unsubs.push(
        await listen<MeetingStatus>("meeting-phase-changed", (event) => {
          setMeetingStatus(event.payload);
          if (event.payload.phase !== "idle") setMeetingError("");
        }),
      );
      unsubs.push(
        await listen<{ app: string; display_name: string }>(
          "meeting-app-detected",
          (event) => {
            setMeetingStatus((prev) => ({
              ...prev,
              phase: prev.phase === "idle" ? "detected" : prev.phase,
              app_name: event.payload.display_name,
            }));
          },
        ),
      );
      unsubs.push(
        await listen("meeting-app-cleared", () => {
          setMeetingStatus((prev) =>
            prev.phase === "detected"
              ? { phase: "idle", app_name: null, started_at: null }
              : prev,
          );
        }),
      );
      unsubs.push(
        await listen<string>("meeting-error", (event) => {
          setMeetingError(event.payload);
        }),
      );
    })();
    return () => {
      window.clearInterval(axTimer);
      window.clearInterval(meetingTimer);
      if (warningTimer.current != null) {
        window.clearTimeout(warningTimer.current);
      }
      for (const u of unsubs) u();
    };
  }, []);

  async function saveConfig() {
    if (!config) return;
    try {
      await invoke("save_user_config", { config });
      setError("");
      // Read back rather than trusting local state: load_config() applies defaults and
      // migrations, so what the app will actually use can differ from what was typed.
      setConfig(await invoke<AppConfig>("get_config"));
      setReadiness(await invoke<string[]>("get_readiness"));
      setVerifyMsg("Settings saved.");
    } catch (err) {
      setVerifyMsg("");
      setError(String(err));
    }
  }

  if (!config || !store) {
    return (
      <div className="hub">
        <p className="muted">Loading Hub…</p>
      </div>
    );
  }

  const processingMode = config.processing_mode.trim().toLowerCase();
  const cloudMode = processingMode === "cloud";
  const hybridMode = processingMode === "hybrid";
  const localMode = !cloudMode && !hybridMode;

  return (
    <div className="hub">
      <aside>
        <div className="hub-brand">
          <img className="hub-logo" src="/flow-icon.png" alt="" width={36} height={36} />
          <div>
            <strong>Flow</strong>
            <span>Vibe Coding · Hub</span>
          </div>
        </div>
        {(
          [
            ["overview", "Overview"],
            ["history", "History"],
            ["meetings", "Meetings"],
            ["dictionary", "Dictionary"],
            ["snippets", "Snippets"],
            ["styles", "Styles"],
            ["settings", "Settings"],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            className={tab === id ? "nav active" : "nav"}
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
        <button
          type="button"
          className="nav hide"
          onClick={() => void invoke("hide_hub_window")}
        >
          Hide
        </button>
      </aside>

      <main>
        {axTrusted === false ? (
          <div className="ready-banner" style={{ borderColor: "var(--danger)", background: "rgba(255,123,114,0.12)" }}>
            <strong>fn hotkey blocked — Accessibility is OFF</strong>
            <span>
              This is why nothing happens when you press fn. System Settings →
              Privacy &amp; Security → Accessibility → turn ON{" "}
              <strong>Flow</strong>, then quit Flow completely and reopen it.
            </span>
            {axStatus?.app_bundle_path || axStatus?.executable_path ? (
              <span>
                Running from{" "}
                <code>{axStatus.app_bundle_path ?? axStatus.executable_path}</code>
              </span>
            ) : null}
            {axStatus?.guidance ? <span>{axStatus.guidance}</span> : null}
            <div className="row" style={{ gap: 8, marginTop: 10, flexWrap: "wrap" }}>
              <button
                type="button"
                className="primary"
                onClick={() => void invoke("open_accessibility_settings")}
              >
                Open Accessibility settings
              </button>
              <button
                type="button"
                className="secondary"
                onClick={() => void invoke("open_microphone_settings")}
              >
                Open Mic settings
              </button>
            </div>
          </div>
        ) : null}
        {meetingStatus.phase !== "idle" ? (
          <div className="ready-banner">
            {meetingStatus.phase === "capturing" ? (
              <>
                <strong>Transcribing {meetingStatus.app_name ?? "call"}…</strong>
                <span>Mic + system audio are being transcribed to text only — no audio is saved.</span>
                <div className="row" style={{ gap: 8, marginTop: 10 }}>
                  <button
                    type="button"
                    className="primary"
                    onClick={async () => {
                      try {
                        await invoke("meeting_stop_capture");
                        setStore(await invoke<FlowStore>("get_store"));
                      } catch (err) {
                        setMeetingError(String(err));
                      }
                    }}
                  >
                    Stop transcription
                  </button>
                </div>
              </>
            ) : screenRecordingTrusted === false ? (
              <>
                <strong>
                  {meetingStatus.app_name ?? "A call"} detected — Screen Recording permission needed
                </strong>
                <span>
                  Capturing the other participants' audio needs Screen Recording permission (audio
                  only — no video or screen content is captured or stored). You can still transcribe
                  your own mic without it.
                </span>
                <div className="row" style={{ gap: 8, marginTop: 10, flexWrap: "wrap" }}>
                  <button
                    type="button"
                    className="secondary"
                    onClick={() => void invoke("open_screen_recording_settings")}
                  >
                    Open Screen Recording settings
                  </button>
                  <button
                    type="button"
                    className="primary"
                    onClick={async () => {
                      try {
                        await invoke("meeting_confirm_notified");
                        await invoke("meeting_start_capture");
                      } catch (err) {
                        setMeetingError(String(err));
                      }
                    }}
                  >
                    I've told the other participants — start (mic only)
                  </button>
                </div>
              </>
            ) : (
              <>
                <strong>{meetingStatus.app_name ?? "A call"} detected</strong>
                <span>
                  Transcription only starts after you confirm you've told the other participants —
                  it never starts automatically.
                </span>
                <div className="row" style={{ gap: 8, marginTop: 10 }}>
                  <button
                    type="button"
                    className="primary"
                    onClick={async () => {
                      try {
                        await invoke("meeting_confirm_notified");
                        await invoke("meeting_start_capture");
                      } catch (err) {
                        setMeetingError(String(err));
                      }
                    }}
                  >
                    I've told the other participants — start transcription
                  </button>
                </div>
              </>
            )}
            {meetingError ? <p className="error">{meetingError}</p> : null}
          </div>
        ) : null}
        {error ? <p className="error">{error}</p> : null}
        {warning ? <p className="warning">{warning}</p> : null}
        {verifyMsg ? <p className="muted">{verifyMsg}</p> : null}
        {readiness.length > 0 ? (
          <div className="ready-banner">
            <strong>Not ready yet</strong>
            <ul>
              {readiness.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          </div>
        ) : (
          <div className="ready-banner ok">
            <strong>Ready</strong>
            <span>
              Hold fn for raw dictation · fn+1 prompt · fn+2 correct · fn+3 answer.
            </span>
          </div>
        )}

        {tab === "overview" ? (
          <section>
            <h1>Overview</h1>
            <p className="muted">
              Primary hotkey (the <strong>fn</strong>/Globe key, bottom-left of the
              keyboard): hold <strong>fn</strong>{" "}
              while speaking for raw dictation; hold <strong>fn</strong> and
              press <strong>1</strong> for auto-prompt; hold <strong>fn</strong> and
              press <strong>2</strong> to correct text; hold <strong>fn</strong> and
              press <strong>3</strong> to answer the latest live question. Enable Microphone + Accessibility
              for <strong>/Applications/Flow.app</strong>.
            </p>
            <p className="muted">
              Processing:{" "}
              {cloudMode
                ? "Cloud — Cloud Run STT + LLM"
                : hybridMode
                  ? "Hybrid — local STT + Cloud Run LLM"
                  : "Local — Mac STT + Mac LLM"}
              . Change this in Settings.
            </p>
            <p className="ready-banner ok" style={{ marginTop: 12 }}>
              <strong>Status:</strong> <span>{liveStatus}</span>
            </p>
            <div className="row" style={{ gap: 12, margin: "16px 0", flexWrap: "wrap" }}>
              <button
                type="button"
                className="secondary"
                onClick={() => void invoke("open_microphone_settings")}
              >
                Mic permission
              </button>
              <button
                type="button"
                className="secondary"
                onClick={() => void invoke("open_accessibility_settings")}
              >
                Accessibility
              </button>
            </div>
            <div className="stats">
              <div>
                <strong>{store.stats.total_dictations}</strong>
                <span>Dictations</span>
              </div>
              <div>
                <strong>{store.stats.total_prompts ?? 0}</strong>
                <span>Prompts</span>
              </div>
              <div>
                <strong>{store.stats.total_words}</strong>
                <span>Words</span>
              </div>
            </div>
            {trainingStats ? (
              <p className="muted" style={{ marginTop: 8 }}>
                Training log: {trainingStats.generations} generations,{" "}
                {trainingStats.with_mistakes} with detected mistakes,{" "}
                {trainingStats.user_corrections} user edits.
              </p>
            ) : null}
            <ul className="list">
              {store.history.slice(0, 5).map((item) => (
                <li key={item.id}>
                  <span className="meta">
                    {item.mode} {item.app_name ? `· ${item.app_name}` : ""}
                  </span>
                  <p>{item.text}</p>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {tab === "history" ? (
          <section>
            <div className="row-head">
              <h1>History</h1>
              <button
                type="button"
                className="secondary"
                onClick={async () => setStore(await invoke("clear_history"))}
              >
                Clear
              </button>
            </div>
            <p className="muted">
              Model training traces (source, draft, detected mistakes, repairs, and later
              edits) are saved to{" "}
              <code>
                {trainingStats?.log_path ||
                  "~/Library/Application Support/voice-flow/training/generations.jsonl"}
              </code>
              . {trainingStats?.generations ?? 0} generations,{" "}
              {trainingStats?.with_mistakes ?? 0} flagged,{" "}
              {trainingStats?.user_corrections ?? 0} user corrections.
            </p>
            <div className="row" style={{ marginBottom: 12 }}>
              <button
                type="button"
                className="secondary"
                onClick={() => void invoke("open_training_folder")}
              >
                Open training folder
              </button>
            </div>
            <ul className="list">
              {store.history.map((item) => (
                <li key={item.id}>
                  <span className="meta">
                    {item.mode} · {item.word_count} words
                    {item.app_name ? ` · ${item.app_name}` : ""}
                  </span>
                  <p>{item.text}</p>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {tab === "meetings" ? (
          <section>
            <div className="row-head">
              <h1>Meetings</h1>
              <button
                type="button"
                className="secondary"
                onClick={async () => setStore(await invoke("clear_meetings"))}
              >
                Clear
              </button>
            </div>
            <p className="muted">
              Transcripts only — no audio is ever saved. Capture always requires confirming you've
              told the other participants first.
            </p>
            <ul className="list">
              {store.meetings.map((meeting) => (
                <li key={meeting.id}>
                  <div className="row-head">
                    <span className="meta">{meeting.app_name}</span>
                    <button
                      type="button"
                      className="secondary"
                      onClick={async () =>
                        setStore(
                          await invoke("remove_meeting_transcript", { id: meeting.id }),
                        )
                      }
                    >
                      Remove
                    </button>
                  </div>
                  {meeting.segments
                    .slice()
                    .sort((a, b) => a.at_ms - b.at_ms)
                    .map((segment, idx) => (
                      <p key={idx}>
                        <strong>{segment.speaker === "you" ? "You" : "Other"}:</strong>{" "}
                        {segment.text}
                      </p>
                    ))}
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {tab === "dictionary" ? (
          <section>
            <h1>Dictionary</h1>
            <p className="muted">
              Names and jargon preferred by Speech-to-Text and English cleanup.
            </p>
            <div className="inline-form">
              <input
                value={word}
                onChange={(e) => setWord(e.target.value)}
                placeholder="Add word or name"
              />
              <button
                type="button"
                className="primary"
                onClick={async () => {
                  setStore(
                    await invoke("add_dictionary_word", { word }),
                  );
                  setWord("");
                }}
              >
                Add
              </button>
            </div>
            <ul className="chips">
              {store.dictionary.map((entry) => (
                <li key={entry.id}>
                  {entry.word}
                  <button
                    type="button"
                    onClick={async () =>
                      setStore(
                        await invoke("remove_dictionary_word", {
                          id: entry.id,
                        }),
                      )
                    }
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {tab === "snippets" ? (
          <section>
            <h1>Snippets</h1>
            <p className="muted">
              Say the trigger (or “snippet trigger”) to expand during dictation.
            </p>
            <div className="stack-form">
              <input
                value={trigger}
                onChange={(e) => setTrigger(e.target.value)}
                placeholder="Trigger phrase"
              />
              <textarea
                value={expansion}
                onChange={(e) => setExpansion(e.target.value)}
                placeholder="Expansion text"
                rows={3}
              />
              <button
                type="button"
                className="primary"
                onClick={async () => {
                  setStore(
                    await invoke("add_snippet", { trigger, expansion }),
                  );
                  setTrigger("");
                  setExpansion("");
                }}
              >
                Add snippet
              </button>
            </div>
            <ul className="list">
              {store.snippets.map((snippet) => (
                <li key={snippet.id}>
                  <div className="row-head">
                    <strong>{snippet.trigger}</strong>
                    <button
                      type="button"
                      className="secondary"
                      onClick={async () =>
                        setStore(
                          await invoke("remove_snippet", { id: snippet.id }),
                        )
                      }
                    >
                      Remove
                    </button>
                  </div>
                  <p>{snippet.expansion}</p>
                </li>
              ))}
            </ul>
          </section>
        ) : null}

        {tab === "styles" ? (
          <section>
            <h1>Styles</h1>
            <p className="muted">Active style shapes English cleanup tone.</p>
            <div className="style-grid">
              {store.styles.map((style) => (
                <button
                  key={style.id}
                  type="button"
                  className={
                    config.active_style_id === style.id
                      ? "style-card active"
                      : "style-card"
                  }
                  onClick={async () => {
                    const next = { ...config, active_style_id: style.id };
                    setConfig(next);
                    await invoke("save_user_config", { config: next });
                  }}
                >
                  <strong>{style.name}</strong>
                  <span>{style.prompt}</span>
                </button>
              ))}
            </div>
          </section>
        ) : null}

        {tab === "settings" ? (
          <section>
            <h1>Settings</h1>
            <div className="stack-form">
              <label>
                Processing mode
                <select
                  value={
                    cloudMode ? "cloud" : hybridMode ? "hybrid" : "local"
                  }
                  onChange={(e) =>
                    setConfig({ ...config, processing_mode: e.target.value })
                  }
                >
                  <option value="cloud">
                    Cloud — Cloud Run STT + LLM
                  </option>
                  <option value="hybrid">
                    Hybrid — local STT + Cloud Run LLM
                  </option>
                  <option value="local">
                    Local — Mac STT + Mac LLM
                  </option>
                </select>
              </label>
              <p className="muted">
                {cloudMode
                  ? "The Mac records and pastes. Speech and prompt processing both run on Cloud Run."
                  : hybridMode
                    ? "The Mac transcribes locally (Whisper or Groq). Cleanup, vibe, grammar, and spoken edits go to Cloud Run. Audio stays on this Mac."
                    : "The Mac transcribes and calls your LLM APIs directly. No Cloud Run required."}
              </p>

              {cloudMode || hybridMode ? (
                <>
                  {config.flow_api_url && config.flow_api_key && !editingCloud ? (
                    <div className="ready-banner ok">
                      <strong>Cloud connected</strong>
                      <span>
                        {hybridMode
                          ? "Cleanup and prompts use Cloud Run. Speech still uses the local provider below."
                          : "Flow is already configured for Cloud Run. You do not need to provide another API key."}
                      </span>
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => setEditingCloud(true)}
                      >
                        Edit cloud connection
                      </button>
                    </div>
                  ) : (
                    <>
                      <label>
                        Cloud Run URL
                        <input
                          value={config.flow_api_url}
                          onChange={(e) =>
                            setConfig({ ...config, flow_api_url: e.target.value })
                          }
                          placeholder="https://flow-api-….run.app"
                        />
                      </label>
                      <label>
                        Cloud Run API key
                        <input
                          type="password"
                          value={config.flow_api_key}
                          onChange={(e) =>
                            setConfig({ ...config, flow_api_key: e.target.value })
                          }
                          placeholder="flow-api-key"
                        />
                      </label>
                      <p className="muted">
                        Only change these when pointing Flow at another deployment.
                        Existing values are stored locally and never shown in full.
                      </p>
                    </>
                  )}
                </>
              ) : null}

              {localMode || hybridMode ? (
                <>
                  <label>
                    Speech provider
                    <select
                      value={config.stt_provider || "local_whisper"}
                      onChange={(e) =>
                        setConfig({ ...config, stt_provider: e.target.value })
                      }
                    >
                      <option value="local_whisper">
                        Local Whisper — no STT API cost
                      </option>
                      <option value="groq_whisper">
                        Groq Whisper — recommended, low Mac load
                      </option>
                      <option value="gcp_speech">
                        Google Speech — legacy, requires GCP billing
                      </option>
                    </select>
                  </label>
                  {(config.stt_provider || "local_whisper") === "gcp_speech" ? (
                    <>
                      <label>
                        GCP project ID
                        <input
                          value={config.gcp_project_id}
                          onChange={(e) =>
                            setConfig({ ...config, gcp_project_id: e.target.value })
                          }
                        />
                      </label>
                      <label>
                        GCP location
                        <input
                          value={config.gcp_location}
                          onChange={(e) =>
                            setConfig({ ...config, gcp_location: e.target.value })
                          }
                        />
                      </label>
                      <label>
                        Speech model
                        <input
                          value={config.stt_model}
                          onChange={(e) =>
                            setConfig({ ...config, stt_model: e.target.value })
                          }
                        />
                      </label>
                      <p className="muted">
                        Google Speech will fail while billing is disabled for the GCP project.
                        Use Groq unless you intentionally re-enable GCP billing.
                      </p>
                    </>
                  ) : (config.stt_provider || "local_whisper") === "groq_whisper" ? (
                    <>
                      <label>
                        Groq API key
                        <input
                          type="password"
                          value={config.groq_api_key}
                          onChange={(e) =>
                            setConfig({ ...config, groq_api_key: e.target.value })
                          }
                          placeholder={
                            config.groq_api_key ? "•••••••• (saved)" : "gsk_…"
                          }
                        />
                      </label>
                      <label>
                        Groq STT model
                        <input
                          value={config.groq_stt_model || "whisper-large-v3-turbo"}
                          onChange={(e) =>
                            setConfig({ ...config, groq_stt_model: e.target.value })
                          }
                        />
                      </label>
                    </>
                  ) : (
                    <>
                      <label>
                        Local Whisper model path
                        <input
                          value={config.local_whisper_model_path}
                          onChange={(e) =>
                            setConfig({
                              ...config,
                              local_whisper_model_path: e.target.value,
                            })
                          }
                          placeholder="/Users/efi/Library/Application Support/voice-flow/models/ggml-small.en.bin"
                        />
                      </label>
                      <p className="muted">
                        Uses `whisper-cli` from Homebrew and `ffmpeg` to transcribe locally.
                        The small English model is more accurate for dictation while still running locally.
                      </p>
                    </>
                  )}
                </>
              ) : null}

              {localMode ? (
                <>
                  <label>
                    LLM provider
                    <select
                      value={config.llm_provider || "deepseek"}
                      onChange={(e) => {
                        const provider = e.target.value;
                        setConfig({
                          ...config,
                          llm_provider: provider,
                          ...llmDefaults(provider),
                          llm_api_key: "",
                        });
                      }}
                    >
                      <option value="deepseek">DeepSeek — cheapest default</option>
                      <option value="xai">xAI Grok — optional</option>
                      <option value="openai_compatible">
                        OpenAI-compatible custom
                      </option>
                    </select>
                  </label>
                  <label>
                    LLM API base URL
                    <input
                      value={config.llm_base_url}
                      onChange={(e) =>
                        setConfig({ ...config, llm_base_url: e.target.value })
                      }
                      placeholder="https://api.deepseek.com"
                    />
                  </label>
                  <label>
                    LLM API key
                    <input
                      type="password"
                      value={config.llm_api_key}
                      onChange={(e) =>
                        setConfig({ ...config, llm_api_key: e.target.value })
                      }
                      placeholder={
                        config.llm_api_key
                          ? "•••••••• (saved)"
                          : (config.llm_provider || "deepseek") === "xai"
                            ? "xai-…"
                            : "sk-…"
                      }
                    />
                  </label>
                  <label>
                    LLM model
                    <input
                      value={config.llm_model}
                      onChange={(e) =>
                        setConfig({
                          ...config,
                          llm_model: e.target.value,
                          correction_model: e.target.value,
                        })
                      }
                    />
                  </label>
                </>
              ) : null}
              <label>
                Vibe project root (folder with context/, skills/, constitutions/)
                <input
                  value={config.vibe_project_root}
                  onChange={(e) =>
                    setConfig({ ...config, vibe_project_root: e.target.value })
                  }
                  placeholder="/Users/…/Wispr Flow"
                />
              </label>
              {localMode || hybridMode ? (
                <p className="muted">
                  {`Speech key: ${
                    (config.stt_provider || "local_whisper") === "gcp_speech"
                      ? "GCP ADC required"
                      : (config.stt_provider || "local_whisper") === "groq_whisper"
                        ? config.groq_api_key
                          ? "present"
                          : "missing"
                        : config.local_whisper_model_path
                          ? "local model present"
                          : "local model missing"
                  }. LLM: ${
                    hybridMode
                      ? "Cloud Run"
                      : config.llm_api_key
                        ? "present"
                        : "missing"
                  }.`}{" "}
                  Keys are stored locally only; never commit them to git.
                </p>
              ) : null}
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={config.correct_english}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      correct_english: e.target.checked,
                    })
                  }
                />
                Light cleanup on dictation (fillers, punctuation, take-backs)
              </label>
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={config.interaction_sounds !== false}
                  onChange={(e) =>
                    setConfig({
                      ...config,
                      interaction_sounds: e.target.checked,
                    })
                  }
                />
                Interaction sounds (start ping / stop cue)
              </label>
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={config.app_aware_tone}
                  onChange={(e) =>
                    setConfig({ ...config, app_aware_tone: e.target.checked })
                  }
                />
                App-aware tone
              </label>
              <label>
                Language
                <select
                  value={config.language}
                  onChange={(e) =>
                    setConfig({ ...config, language: e.target.value })
                  }
                >
                  {LANGS.map((lang) => (
                    <option key={lang.id} value={lang.id}>
                      {lang.label}
                    </option>
                  ))}
                </select>
              </label>
              <p className="muted">
                Hold <strong>fn</strong> = raw dictation.{" "}
                <strong>fn+1</strong> = prompt from current text.{" "}
                <strong>fn+2</strong> = correct current text.{" "}
                <strong>fn+3</strong> = answer the latest live-conversation question.
              </p>
              <button type="button" className="primary" onClick={saveConfig}>
                Save settings
              </button>
              {cloudMode || hybridMode ? (
                <button
                  type="button"
                  className="secondary"
                  onClick={async () => {
                    setVerifyMsg("Checking Cloud Run…");
                    setError("");
                    try {
                      setVerifyMsg(await invoke<string>("verify_llm_connection"));
                    } catch (err) {
                      setVerifyMsg("");
                      setError(String(err));
                    }
                  }}
                >
                  Verify Cloud Run
                </button>
              ) : null}
              {localMode ? (
                <button
                  type="button"
                  className="secondary"
                  onClick={async () => {
                    setVerifyMsg("Importing DeepSeek key from gcloud…");
                    setError("");
                    try {
                      setVerifyMsg(
                        await invoke<string>("import_deepseek_from_gcloud"),
                      );
                      await refresh();
                    } catch (err) {
                      setVerifyMsg("");
                      setError(String(err));
                    }
                  }}
                >
                  Import DeepSeek key from Secret Manager
                </button>
              ) : null}
              {localMode || hybridMode ? (
                <button
                  type="button"
                  className="secondary"
                  onClick={async () => {
                    setVerifyMsg("Checking Speech provider…");
                    setError("");
                    try {
                      setVerifyMsg(
                        await invoke<string>("verify_stt_connection"),
                      );
                    } catch (err) {
                      setVerifyMsg("");
                      setError(String(err));
                    }
                  }}
                >
                  Verify Speech
                </button>
              ) : null}
              {localMode ? (
                <button
                  type="button"
                  className="secondary"
                  onClick={async () => {
                    setVerifyMsg("Checking LLM…");
                    setError("");
                    try {
                      setVerifyMsg(
                        await invoke<string>("verify_llm_connection"),
                      );
                    } catch (err) {
                      setVerifyMsg("");
                      setError(String(err));
                    }
                  }}
                >
                  Verify LLM
                </button>
              ) : null}
            </div>
          </section>
        ) : null}
      </main>
    </div>
  );
}
