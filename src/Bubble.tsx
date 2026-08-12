import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  currentMonitor,
  cursorPosition,
  getCurrentWindow,
  LogicalPosition,
  monitorFromPoint,
  primaryMonitor,
} from "@tauri-apps/api/window";
import type { MeetingStatus, Status } from "./lib/types";

function formatElapsed(startedAtEpochSecs: string | null): string {
  if (!startedAtEpochSecs) return "";
  const startedMs = Number(startedAtEpochSecs) * 1000;
  if (!Number.isFinite(startedMs)) return "";
  const elapsedSec = Math.max(0, Math.floor((Date.now() - startedMs) / 1000));
  const mm = String(Math.floor(elapsedSec / 60)).padStart(2, "0");
  const ss = String(elapsedSec % 60).padStart(2, "0");
  return `${mm}:${ss}`;
}

type Mode = "vibe" | "vibe_refine" | "dictate" | "command" | "prompt";

const DOCK_WIDTH = 72;
const DOCK_HEIGHT = 200;
const DOCK_MARGIN = 28;

/** Prefer the monitor under the mouse — so the pill follows you across screens. */
async function monitorUnderCursor() {
  try {
    const cursor = await cursorPosition();
    const hit = await monitorFromPoint(cursor.x, cursor.y);
    if (hit) return hit;
  } catch {
    // Fall through if cursor APIs are unavailable.
  }
  return (await currentMonitor()) ?? (await primaryMonitor());
}

async function anchorDockToRightEdge(): Promise<void> {
  const win = getCurrentWindow();
  const monitor = await monitorUnderCursor();
  if (monitor) {
    const scale = monitor.scaleFactor;
    const screenX = monitor.position.x / scale;
    const screenY = monitor.position.y / scale;
    const screenWidth = monitor.size.width / scale;
    const screenHeight = monitor.size.height / scale;
    const targetX = Math.round(screenX + screenWidth - DOCK_WIDTH - DOCK_MARGIN);
    const targetY = Math.round(screenY + (screenHeight - DOCK_HEIGHT) / 2);
    const position = await win.outerPosition();
    const windowScale = await win.scaleFactor();
    const currentX = position.x / windowScale;
    const currentY = position.y / windowScale;
    if (Math.abs(currentX - targetX) > 1 || Math.abs(currentY - targetY) > 1) {
      await win.setPosition(new LogicalPosition(targetX, targetY));
    }
  }
  // Do NOT call setAlwaysOnTop / setVisibleOnAllWorkspaces from JS — those Tauri
  // setters reset NSWindow level back to floating and the dock vanishes under Cursor.
  // Rust configure_dock_overlay owns overlay level + Space joining.
  if (!(await win.isVisible())) {
    await win.show();
  }
}

function isProcessing(status: Status): boolean {
  return (
    status === "recording" ||
    status === "transcribing" ||
    status === "correcting" ||
    status === "pasting"
  );
}

export default function Bubble() {
  const [status, setStatus] = useState<Status>("idle");
  const [mode, setMode] = useState<Mode>("vibe");
  const [error, setError] = useState("");
  const [meetingStatus, setMeetingStatus] = useState<MeetingStatus>({
    phase: "idle",
    app_name: null,
    started_at: null,
  });
  const [elapsed, setElapsed] = useState("");

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
    let unsubs: Array<() => void> = [];
    const keepAnchored = () => void anchorDockToRightEdge().catch(() => undefined);
    keepAnchored();
    // Follow the cursor's screen closely so the pill stays visible wherever you type.
    const anchorTimer = window.setInterval(keepAnchored, 750);

    (async () => {
      try {
        await anchorDockToRightEdge();
      } catch {
        // Positioning is best-effort; dock still renders.
      }

      unsubs.push(
        await listen<string>("recording-start", (event) => {
          const next: Mode =
            event.payload === "vibe_refine"
              ? "vibe_refine"
              : event.payload === "vibe"
                ? "vibe"
                : event.payload === "command"
                  ? "command"
                  : event.payload === "prompt"
                    ? "prompt"
                    : "vibe";
          setMode(next);
          setError("");
          setStatus("recording");
        }),
      );
      unsubs.push(
        await listen<string>("session-mode", (event) => {
          if (event.payload === "vibe_refine") {
            setMode("vibe_refine");
            setError("");
            setStatus("correcting");
          } else if (event.payload === "vibe") {
            setMode("vibe");
          }
        }),
      );
      unsubs.push(
        await listen<string>("recording-stop", () => {
          setStatus("transcribing");
        }),
      );
      unsubs.push(
        await listen<string>("dictation-status", (event) => {
          const value = event.payload;
          if (
            value === "transcribing" ||
            value === "correcting" ||
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
            if (value === "idle") setError("");
          }
        }),
      );
      unsubs.push(
        await listen<string>("dictation-error", (event) => {
          setStatus("error");
          setError(event.payload);
        }),
      );
      unsubs.push(
        await listen<string>("dictation-done", () => {
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
      window.clearInterval(anchorTimer);
      unsubs.forEach((fn) => fn());
    };
  }, []);

  const meetingActive = meetingStatus.phase === "capturing";
  const processing = isProcessing(status);
  const title = meetingActive
    ? `Transcribing ${meetingStatus.app_name ?? "meeting"}… ${elapsed} — text only, no audio saved`
    : status === "error"
      ? error || "Error"
      : status === "recording"
        ? "Listening…"
        : status === "transcribing"
          ? "Transcribing…"
          : status === "correcting"
            ? mode === "vibe_refine"
              ? "Refining…"
              : "Processing…"
            : status === "pasting"
              ? "Pasting…"
              : "Flow — hold fn · fn+1 auto-prompt · fn+2 refine";

  return (
    <div
      className={`dock ${status}${processing ? " processing" : ""}${meetingActive ? " meeting-active" : ""}`}
      title={title}
      role="status"
      aria-label={title}
    >
      <div className="dock-pill">
        {processing ? <span className="dock-loader" aria-hidden /> : null}
        {status === "error" ? <span className="dock-error-fill" aria-hidden /> : null}
        {meetingActive ? <span className="dock-meeting-dot" aria-hidden /> : null}
      </div>
    </div>
  );
}
