import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MeetingStatus, Status } from "./lib/types";
import { useDictationEngine } from "./lib/useDictationEngine";

function formatElapsed(startedAtEpochSecs: string | null): string {
  if (!startedAtEpochSecs) return "";
  const startedMs = Number(startedAtEpochSecs) * 1000;
  if (!Number.isFinite(startedMs)) return "";
  const elapsedSec = Math.max(0, Math.floor((Date.now() - startedMs) / 1000));
  const mm = String(Math.floor(elapsedSec / 60)).padStart(2, "0");
  const ss = String(elapsedSec % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}

type Mode =
  | "vibe_text"
  | "correct_text"
  | "meeting_answer"
  | "dictate";
type BarState = "idle" | "listening" | "processing" | "error";
type VisualState = BarState;

const WAVE_BARS = 9;

function parseMode(payload: string): Mode {
  if (payload === "vibe_text") return "vibe_text";
  if (payload === "correct_text") return "correct_text";
  if (payload === "meeting_answer") return "meeting_answer";
  if (payload === "dictate") return "dictate";
  return "dictate";
}

function barStateFromStatus(status: Status): BarState {
  if (status === "recording") return "listening";
  if (
    status === "transcribing" ||
    status === "correcting" ||
    status === "answering" ||
    status === "pasting"
  ) {
    return "processing";
  }
  if (status === "error") return "error";
  return "idle";
}

async function layoutDock(layout: "idle" | "listening"): Promise<void> {
  // Rust owns size, click-through, position, window level, and Spaces behavior.
  // Keeping one owner avoids competing JS/native keepalive loops.
  await invoke("set_bubble_layout", { layout });
}

function waveHeights(level: number): number[] {
  // Diamond envelope (tallest center) with slight phase wobble — matches the pill visualizer.
  const floor = 0.16;
  const center = (WAVE_BARS - 1) / 2;
  return Array.from({ length: WAVE_BARS }, (_, i) => {
    const dist = center === 0 ? 0 : Math.abs(i - center) / center;
    const envelope = 1 - dist * 0.62;
    const wobble = 0.55 + 0.45 * Math.sin(level * 9 + i * 1.15);
    return Math.min(1, Math.max(floor, floor + level * envelope * wobble));
  });
}

function MicIcon() {
  return (
    <svg
      className="flow-bar-mic-icon"
      viewBox="0 0 24 24"
      width="18"
      height="18"
      aria-hidden
    >
      <path
        fill="currentColor"
        d="M12 14a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v5a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2Z"
      />
    </svg>
  );
}

export default function Bubble() {
  const [status, setStatus] = useState<Status>("idle");
  const [mode, setMode] = useState<Mode>("dictate");
  const [error, setError] = useState("");
  const [micLevel, setMicLevel] = useState(0);
  const [meetingStatus, setMeetingStatus] = useState<MeetingStatus>({
    phase: "idle",
    app_name: null,
    started_at: null,
  });
  const [elapsed, setElapsed] = useState("");
  const statusRef = useRef<Status>("idle");

  // The always-visible bubble owns dictation capture. Hidden WKWebViews can leave
  // getUserMedia pending forever even when macOS Microphone permission is granted.
  useDictationEngine(true);

  const barState = barStateFromStatus(status);
  const visualState: VisualState = barState;
  const dockLayout = visualState === "listening" ? "listening" : "idle";
  const heights = useMemo(() => waveHeights(micLevel), [micLevel]);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  useEffect(() => {
    if (meetingStatus.phase !== "capturing") {
      setElapsed("");
      return;
    }
    const tick = () => setElapsed(formatElapsed(meetingStatus.started_at));
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [meetingStatus.phase, meetingStatus.started_at]);

  useEffect(() => {
    void layoutDock(dockLayout).catch(() => undefined);
  }, [dockLayout]);

  useEffect(() => {
    let unsubs: Array<() => void> = [];

    (async () => {
      try {
        await layoutDock("idle");
      } catch {
        // Positioning is best-effort; dock still renders.
      }

      unsubs.push(
        await listen<string>("recording-start", (event) => {
          const next = parseMode(event.payload);
          setMode(next);
          setError("");
          setMicLevel(0);
          setStatus("recording");
        }),
      );
      unsubs.push(
        await listen<string>("session-mode", (event) => {
          if (event.payload === "vibe_text") {
            setMode("vibe_text");
            setError("");
            setStatus("correcting");
          } else if (event.payload === "correct_text") {
            setMode("correct_text");
            setError("");
            setStatus("correcting");
          } else if (event.payload === "meeting_answer") {
            setMode("meeting_answer");
            setError("");
            setStatus("answering");
          }
        }),
      );
      unsubs.push(
        await listen<string>("recording-stop", () => {
          // Keep the waveform visible until the recorder has actually stopped.
          // `useDictationEngine` emits `engine-status: transcribing` once it has
          // finalized audio and started cloud/local processing.
        }),
      );
      unsubs.push(
        await listen<string>("recording-cancel", (event) => {
          if (
            event.payload === "vibe_text" ||
            event.payload === "correct_text" ||
            event.payload === "meeting_answer"
          ) {
            return;
          }
          setMicLevel(0);
          setError("");
          setStatus("idle");
        }),
      );
      unsubs.push(
        await listen<number>("mic-level", (event) => {
          const value = Number(event.payload);
          if (Number.isFinite(value)) {
            setMicLevel(Math.min(1, Math.max(0, value)));
          }
        }),
      );
      unsubs.push(
        await listen<string>("dictation-status", (event) => {
          const value = event.payload;
          if (
            value === "transcribing" ||
            value === "correcting" ||
            value === "answering" ||
            value === "pasting"
          ) {
            setStatus(value);
          }
        }),
      );
      unsubs.push(
        await listen<string>("engine-status", (event) => {
          const value = event.payload;
          if (
            value === "idle" ||
            value === "recording" ||
            value === "transcribing" ||
            value === "error"
          ) {
            setStatus(value);
            if (value === "idle" || value === "error") setMicLevel(0);
            if (value === "idle") setError("");
          }
        }),
      );
      unsubs.push(
        await listen<string>("dictation-error", (event) => {
          setMicLevel(0);
          setStatus("error");
          setError(event.payload);
          window.setTimeout(() => {
            if (statusRef.current === "error") {
              setStatus("idle");
              setError("");
            }
          }, 4500);
        }),
      );
      unsubs.push(
        await listen<string>("dictation-warning", (event) => {
          // Keep warnings available for the tooltip/Hub, but do not replace
          // the listening UI with a red error pill.
          setError(event.payload);
          window.setTimeout(() => {
            if (statusRef.current !== "error") {
              setError("");
            }
          }, 4500);
        }),
      );
      unsubs.push(
        await listen<string>("dictation-done", () => {
          setMicLevel(0);
          setStatus("idle");
          setError("");
        }),
      );
      unsubs.push(
        await listen<MeetingStatus>("meeting-phase-changed", (event) => {
          setMeetingStatus(event.payload);
        }),
      );
    })();

    return () => {
      unsubs.forEach((fn) => fn());
    };
  }, []);

  const meetingActive = meetingStatus.phase === "capturing";
  const title = meetingActive
    ? `Transcribing ${meetingStatus.app_name ?? "meeting"}… ${elapsed} — text only, no audio saved`
    : barState === "error"
      ? error || "Error"
      : barState === "listening"
        ? "Listening… · Stop ■ · Cancel ×"
        : barState === "processing"
          ? mode === "vibe_text"
            ? "Creating prompt…"
            : mode === "correct_text"
              ? "Correcting…"
              : mode === "meeting_answer"
                ? "Answering…"
            : status === "pasting"
              ? "Inserting…"
              : "Processing…"
          : "Flow — fn raw · fn+1 prompt · fn+2 correct · fn+3 answer";

  async function onStop(e: React.MouseEvent<HTMLButtonElement>) {
    e.stopPropagation();
    try {
      await invoke("flow_bar_stop", { mode });
    } catch {
      // Session may already be ending.
    }
  }

  async function onCancel(e: React.MouseEvent<HTMLButtonElement>) {
    e.stopPropagation();
    try {
      await invoke("flow_bar_cancel");
    } catch {
      // Session may already be cancelled.
    }
  }

  return (
    <div
      className={`flow-bar state-${visualState}${meetingActive ? " meeting-active" : ""}`}
      title={title}
      role="status"
      aria-label={title}
    >
      <div className="flow-bar-shell">
        {barState === "idle" || barState === "error" ? (
          <div className="flow-bar-idle-mark" aria-hidden>
            {barState === "error" ? (
              <span className="flow-bar-error-bang">!</span>
            ) : (
              <span className="flow-bar-mic">
                <MicIcon />
              </span>
            )}
          </div>
        ) : null}

        {barState === "listening" ? (
          <>
            <span className="flow-bar-mic listening" aria-hidden>
              <MicIcon />
            </span>
            <div className="flow-bar-wave" aria-hidden>
              {heights.map((h, i) => (
                <span
                  key={i}
                  className="flow-bar-wave-bar"
                  style={{ width: `${Math.round(h * 100)}%` }}
                />
              ))}
            </div>
            <button
              type="button"
              className="flow-bar-btn cancel"
              aria-label="Cancel recording"
              onClick={onCancel}
            >
              ×
            </button>
            <button
              type="button"
              className="flow-bar-btn stop"
              aria-label="Stop and process"
              onClick={onStop}
            >
              <span className="flow-bar-stop-sq" />
            </button>
          </>
        ) : null}

        {barState === "processing" ? (
          <>
            <span className="flow-bar-mic processing" aria-hidden>
              <MicIcon />
            </span>
            <div className="flow-bar-spinner" aria-hidden />
          </>
        ) : null}

        {meetingActive ? <span className="flow-bar-meeting-dot" aria-hidden /> : null}
      </div>
    </div>
  );
}
