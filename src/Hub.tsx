import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AccessibilityStatus,
  AppConfig,
  FlowStore,
  MeetingStatus,
} from "./lib/types";
import { startRecording, stopRecording } from "./lib/audio";
import { useDictationEngine } from "./lib/useDictationEngine";
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

export default function Hub() {
  const [tab, setTab] = useState<Tab>("overview");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [store, setStore] = useState<FlowStore | null>(null);
  const [error, setError] = useState("");
  const [warning, setWarning] = useState("");
  const [verifyMsg, setVerifyMsg] = useState("");
  const [readiness, setReadiness] = useState<string[]>([]);
  const [word, setWord] = useState("");
  const [trigger, setTrigger] = useState("");
  const [expansion, setExpansion] = useState("");
  const [liveStatus, setLiveStatus] = useState("Idle");
  const [recording, setRecording] = useState(false);
  // A hotkey (fn) session is driven by useDictationEngine, which owns the same
  // module-level recorder this panel's button uses. Tracked separately so the button
  // can stand down instead of calling stopRecording() underneath the engine.
  const [hotkeySession, setHotkeySession] = useState(false);
  const didInitialRoute = useRef(false);
  const [axTrusted, setAxTrusted] = useState<boolean | null>(null);
  const [axStatus, setAxStatus] = useState<AccessibilityStatus | null>(null);
  const [meetingStatus, setMeetingStatus] = useState<MeetingStatus>({
    phase: "idle",
    app_name: null,
    started_at: null,
  });
  const [meetingError, setMeetingError] = useState("");
  const [screenRecordingTrusted, setScreenRecordingTrusted] = useState<boolean | null>(null);
  const recordingBusy = useRef(false);

  // Main window owns the mic pipeline so hotkeys work before Hub is visible.
  useDictationEngine(true);
  // Owns the mic-chunk restart loop for meeting transcription, reactive to backend phase.
  useMeetingCapture();

  async function toggleHubRecording() {
    if (recordingBusy.current) return;
    recordingBusy.current = true;
    try {
      if (!recording) {
        setError("");
        setLiveStatus("Requesting microphone…");
        await startRecording();
        setRecording(true);
        setLiveStatus("Recording… tap again to stop");
        return;
      }
      setLiveStatus("Stopping…");
      const audio = await stopRecording();
      setRecording(false);
      if (!audio) {
        setError("No audio captured. Click Mic permission, enable Flow, then try again.");
        setLiveStatus("Idle");
        return;
      }
      setLiveStatus("Sending to MyGCP (STT + prompt)… this can take 1–2 min");
      const text = await invoke<string>("process_dictation", {
        audioBase64: audio,
        mode: "vibe",
      });
      setLiveStatus("Done — prompt is on the clipboard (⌘V)");
      setWarning(text.slice(0, 280) + (text.length > 280 ? "…" : ""));
      await refresh();
    } catch (err) {
      setRecording(false);
      setError(String(err));
      setLiveStatus("Error");
      try {
        await stopRecording();
      } catch {
        /* ignore */
      }
    } finally {
      recordingBusy.current = false;
    }
  }

  async function refresh() {
    const [cfg, st, ready] = await Promise.all([
      invoke<AppConfig>("get_config"),
      invoke<FlowStore>("get_store"),
      invoke<string[]>("get_readiness"),
    ]);
    setConfig(cfg);
    setStore(st);
    setReadiness(ready);
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
          setHotkeySession(true);
        }),
      );
      unsubs.push(
        await listen("recording-stop", () => {
          setLiveStatus("Hotkey: processing…");
          setHotkeySession(false);
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

  // Mirrors cloud_api::uses_cloud on the Rust side.
  const cloudMode = config.processing_mode.trim().toLowerCase() === "cloud";

  return (
    <div className="hub">
      <aside>
        <div className="hub-brand">
          <strong>Flow</strong>
          <span>Vibe Coding · Hub</span>
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
              Until then, use the green <strong>Start recording</strong> button
              below (needs Mic only).
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
              Hold fn to activate · fn+1 auto-prompt · fn+2 refine. Or use Start
              recording below.
            </span>
          </div>
        )}

        {tab === "overview" ? (
          <section>
            <h1>Overview</h1>
            <p className="muted">
              Primary hotkey (the <strong>fn</strong>/Globe key, bottom-left of the
              keyboard): hold <strong>fn</strong>{" "}
              while speaking to activate (grammar paste); hold <strong>fn</strong> and
              press <strong>1</strong> for auto-prompt; hold <strong>fn</strong> and
              press <strong>2</strong> to refine. Enable Microphone + Accessibility
              for <strong>/Applications/Flow.app</strong>.
            </p>
            <p className="ready-banner ok" style={{ marginTop: 12 }}>
              <strong>Status:</strong> <span>{liveStatus}</span>
            </p>
            <div className="row" style={{ gap: 12, margin: "16px 0", flexWrap: "wrap" }}>
              <button
                type="button"
                className="primary"
                disabled={hotkeySession}
                onClick={() => void toggleHubRecording()}
              >
                {hotkeySession
                  ? "Hotkey session running…"
                  : recording
                    ? "Stop & generate prompt"
                    : "Start recording"}
              </button>
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
              <button
                type="button"
                className="secondary"
                onClick={async () => {
                  try {
                    setWarning(await invoke<string>("quit_wispr_flow_conflict"));
                  } catch (e) {
                    setError(String(e));
                  }
                }}
              >
                Quit Wispr Flow conflict
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
                <strong>{store.stats.total_commands}</strong>
                <span>Commands</span>
              </div>
              <div>
                <strong>{store.stats.total_words}</strong>
                <span>Words</span>
              </div>
            </div>
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
                  value={cloudMode ? "cloud" : "local"}
                  onChange={(e) =>
                    setConfig({ ...config, processing_mode: e.target.value })
                  }
                >
                  <option value="cloud">
                    MyGCP Cloud Run — Speech + Qwen2.5-3B
                  </option>
                  <option value="local">
                    Local Mac → GCP Speech + DeepSeek
                  </option>
                </select>
              </label>
              <p className="muted">
                {cloudMode
                  ? "The Mac only records and pastes; Speech-to-Text and the prompt model both run on MyGCP."
                  : "The Mac calls GCP Speech and DeepSeek directly. Needs gcloud application-default credentials and a DeepSeek key."}
              </p>

              {cloudMode ? (
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
                      placeholder={
                        config.flow_api_key ? "•••••••• (saved)" : "flow-api-key"
                      }
                    />
                  </label>
                  <p className="muted">
                    Both are written by <code>bash cloud/deploy.sh</code>. Paste them
                    here only if you are pointing Flow at a different deployment.
                  </p>
                </>
              ) : (
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
                  <label>
                    DeepSeek API key
                    <input
                      type="password"
                      value={config.llm_api_key}
                      onChange={(e) =>
                        setConfig({ ...config, llm_api_key: e.target.value })
                      }
                      placeholder={
                        config.llm_api_key ? "•••••••• (saved)" : "sk-…"
                      }
                    />
                  </label>
                  <label>
                    DeepSeek model
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
              )}
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
              <p className="muted">
                {cloudMode
                  ? `Cloud Run key: ${config.flow_api_key ? "present" : "missing"}.`
                  : `DeepSeek key: ${
                      config.llm_api_key
                        ? `present (${config.llm_api_key.slice(0, 6)}…)`
                        : "missing"
                    }.`}{" "}
                Prefer importing from Secret Manager — never commit keys to git.
              </p>
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={config.hands_free}
                  onChange={(e) =>
                    setConfig({ ...config, hands_free: e.target.checked })
                  }
                />
                Hands-free (tap Control+1 to start/stop instead of holding it)
              </label>
              <p className="muted">
                Applies to the Control+1 fallback only. The primary fn hotkey is
                always hold-to-talk. Grammar cleanup runs before the Vibe prompt
                is generated, in every mode.
              </p>
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
                Hold <strong>fn</strong> (or Control+1) = speech → text → grammar →
                perfect Vibe Coding prompt. <strong>fn+2</strong> (or Control+2) =
                select the prompt → load context/ → refined prompt.
              </p>
              <button type="button" className="primary" onClick={saveConfig}>
                Save settings
              </button>
              {cloudMode ? (
                // In cloud mode both verify commands resolve to the same Cloud Run
                // /health probe, so offering two buttons only implied two checks.
                <button
                  type="button"
                  className="secondary"
                  onClick={async () => {
                    setVerifyMsg("Checking MyGCP Cloud Run…");
                    setError("");
                    try {
                      setVerifyMsg(await invoke<string>("verify_stt_connection"));
                    } catch (err) {
                      setVerifyMsg("");
                      setError(String(err));
                    }
                  }}
                >
                  Verify Cloud Run
                </button>
              ) : (
                <>
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
                  <button
                    type="button"
                    className="secondary"
                    onClick={async () => {
                      setVerifyMsg("Checking GCP Speech…");
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
                    Verify Speech API
                  </button>
                  <button
                    type="button"
                    className="secondary"
                    onClick={async () => {
                      setVerifyMsg("Checking DeepSeek…");
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
                    Verify DeepSeek
                  </button>
                </>
              )}
            </div>
          </section>
        ) : null}
      </main>
    </div>
  );
}
