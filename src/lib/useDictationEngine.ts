import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { cancelRecording, startRecording, stopRecording } from "./audio";
import {
  playCancelSound,
  playStartPing,
  playStopSound,
} from "./interactionSounds";
import type { UiPrefs } from "./types";

type Mode =
  | "vibe_text"
  | "correct_text"
  | "meeting_answer"
  | "dictate";

function parseMode(payload: string): Mode {
  if (payload === "vibe_text") return "vibe_text";
  if (payload === "correct_text") return "correct_text";
  if (payload === "meeting_answer") return "meeting_answer";
  if (payload === "dictate") return "dictate";
  return "dictate";
}

async function soundsEnabled(): Promise<boolean> {
  try {
    const prefs = await invoke<UiPrefs>("get_ui_prefs");
    return prefs.interaction_sounds !== false;
  } catch {
    return true;
  }
}

async function logDictationError(message: string): Promise<void> {
  try {
    await invoke("log_dictation_error", { message });
  } catch {
    // Logging must not create another user-facing error.
  }
}

async function resetNativeRecordingSession(): Promise<void> {
  try {
    await invoke("reset_recording_session");
  } catch {
    // The UI still needs to recover even if the native reset command is unavailable.
  }
}

function canUseProgressiveStt(prefs: UiPrefs, mode: Mode): boolean {
  const processing = prefs.processing_mode.trim().toLowerCase();
  return (
    processing !== "cloud" &&
    mode !== "vibe_text" &&
    mode !== "correct_text" &&
    mode !== "meeting_answer"
  );
}

function isNoSpeechError(message: string): boolean {
  const text = message.toLowerCase();
  return (
    text.includes("no audio captured") ||
    text.includes("no speech detected") ||
    text.includes("empty text") ||
    text.includes("audio too short") ||
    text.includes("gcp speech returned empty text") ||
    text.includes("cloud stt returned empty text") ||
    text.includes("cloud process returned empty text")
  );
}

/**
 * Runs in the always-visible bubble window. Hidden WKWebViews can leave
 * getUserMedia pending forever even when macOS Microphone permission is granted.
 */
export function useDictationEngine(enabled: boolean) {
  const busyRef = useRef(false);
  const openingRef = useRef(false);
  const pendingActionRef = useRef<"stop" | "cancel" | null>(null);
  const modeRef = useRef<Mode>("dictate");
  const partialTranscriptRef = useRef("");
  const partialBusyRef = useRef(false);
  const partialQueueRef = useRef<string[]>([]);
  const partialSessionRef = useRef(0);
  const sttTranscriptRef = useRef("");
  const sttBusyRef = useRef(false);
  const sttQueueRef = useRef<string[]>([]);

  useEffect(() => {
    if (!enabled) return;

    let unlistenStart: (() => void) | undefined;
    let unlistenStop: (() => void) | undefined;
    let unlistenMode: (() => void) | undefined;
    let unlistenCancel: (() => void) | undefined;

    (async () => {
      unlistenMode = await listen<string>("session-mode", (event) => {
        modeRef.current = parseMode(event.payload);
      });

      const finishRecording = async (mode: Mode) => {
        if (busyRef.current) return;
        busyRef.current = true;
        try {
          if (await soundsEnabled()) playStopSound();
          const session = partialSessionRef.current;
          const audio = await stopRecording({ flushPartial: true });
          const drained = await waitForSttDrain(session, 2500);
          await waitForPartialDrain(session, 1800);
          const sttTranscript = sttTranscriptRef.current.trim();
          const partialTranscript = partialTranscriptRef.current.trim();
          partialSessionRef.current += 1;
          partialQueueRef.current = [];
          sttQueueRef.current = [];
          if (
            !audio &&
            !sttTranscript &&
            !partialTranscript &&
            mode !== "vibe_text" &&
            mode !== "correct_text" &&
            mode !== "meeting_answer"
          ) {
            await emit("dictation-warning", "No speech detected.");
            await emit("engine-status", "idle");
            await invoke("hide_bubble_window");
            return;
          }
          await emit("engine-status", "transcribing");
          // Prefer stitched ~50s STT chunks so long holds never hit the 60s
          // GCP sync cap. If the last chunk is still in flight, fall back to the
          // full recording instead of pasting a truncated transcript.
          const stitched = drained
            ? sttTranscript || partialTranscript
            : "";
          const text = stitched
            ? await invoke<string>("process_transcript", {
                transcript: stitched,
                mode,
              })
            : await invoke<string>("process_dictation", {
                audioBase64: audio || "",
                mode,
              });
          await emit("partial-transcript", text);
          await emit("engine-status", "idle");
        } catch (err) {
          const message = String(err);
          if (isNoSpeechError(message)) {
            await emit("dictation-warning", "No speech detected.");
            await emit("engine-status", "idle");
          } else {
            await logDictationError(message);
            await emit("dictation-error", message);
            await emit("engine-status", "error");
          }
          await invoke("hide_bubble_window").catch(() => undefined);
        } finally {
          busyRef.current = false;
        }
      };

      const cancelCurrentRecording = async () => {
        partialSessionRef.current += 1;
        partialTranscriptRef.current = "";
        partialQueueRef.current = [];
        sttTranscriptRef.current = "";
        sttQueueRef.current = [];
        await cancelRecording();
        if (await soundsEnabled()) playCancelSound();
        await emit("mic-level", 0);
        await emit("engine-status", "idle");
      };

      const drainPartialQueue = async (session: number) => {
        if (partialBusyRef.current) return;
        partialBusyRef.current = true;
        try {
          while (
            partialQueueRef.current.length > 0 &&
            partialSessionRef.current === session
          ) {
            const chunk = partialQueueRef.current.shift();
            if (!chunk) continue;
            try {
              const text = await invoke<string>("transcribe_partial", {
                audioBase64: chunk,
              });
              if (partialSessionRef.current !== session) return;
              const clean = text.trim();
              if (!clean) continue;
              partialTranscriptRef.current = [
                partialTranscriptRef.current.trim(),
                clean,
              ]
                .filter(Boolean)
                .join(" ");
              await emit("partial-transcript", partialTranscriptRef.current);
            } catch (err) {
              if (!isNoSpeechError(String(err))) {
                await logDictationError(`Partial STT failed: ${err}`);
              }
            }
          }
        } finally {
          partialBusyRef.current = false;
        }
      };

      const waitForPartialDrain = async (session: number, timeoutMs: number) => {
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline && partialSessionRef.current === session) {
          if (!partialBusyRef.current && partialQueueRef.current.length === 0) return;
          await new Promise((resolve) => window.setTimeout(resolve, 50));
        }
      };

      const drainSttQueue = async (session: number) => {
        if (sttBusyRef.current) return;
        sttBusyRef.current = true;
        try {
          while (
            sttQueueRef.current.length > 0 &&
            partialSessionRef.current === session
          ) {
            const chunk = sttQueueRef.current.shift();
            if (!chunk) continue;
            try {
              const text = await invoke<string>("transcribe_partial", {
                audioBase64: chunk,
              });
              if (partialSessionRef.current !== session) return;
              const clean = text.trim();
              if (!clean) continue;
              sttTranscriptRef.current = [sttTranscriptRef.current.trim(), clean]
                .filter(Boolean)
                .join(" ");
              await emit("partial-transcript", sttTranscriptRef.current);
            } catch (err) {
              if (!isNoSpeechError(String(err))) {
                await logDictationError(`Chunk STT failed: ${err}`);
              }
            }
          }
        } finally {
          sttBusyRef.current = false;
        }
      };

      const waitForSttDrain = async (session: number, timeoutMs: number) => {
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline && partialSessionRef.current === session) {
          if (!sttBusyRef.current && sttQueueRef.current.length === 0) return true;
          await new Promise((resolve) => window.setTimeout(resolve, 50));
        }
        return !sttBusyRef.current && sttQueueRef.current.length === 0;
      };

      unlistenStart = await listen<string>("recording-start", async (event) => {
        if (busyRef.current || openingRef.current) {
          await resetNativeRecordingSession();
          // The previous session is still transcribing. Say so — silently dropping
          // the session made the hotkey look broken with no explanation.
          await emit(
            "dictation-error",
            "Still processing the previous dictation. Wait for it to finish, then try again.",
          );
          await emit("engine-status", "error");
          return;
        }
        modeRef.current = parseMode(event.payload);
        openingRef.current = true;
        pendingActionRef.current = null;
        partialSessionRef.current += 1;
        partialTranscriptRef.current = "";
        partialQueueRef.current = [];
        sttTranscriptRef.current = "";
        sttQueueRef.current = [];
        try {
          const session = partialSessionRef.current;
          const prefs = await invoke<UiPrefs>("get_ui_prefs");
          await startRecording({
            onPartialChunk: canUseProgressiveStt(prefs, modeRef.current)
              ? (base64) => {
                  if (partialSessionRef.current !== session) return;
                  partialQueueRef.current.push(base64);
                  void drainPartialQueue(session);
                }
              : undefined,
            onSttChunk:
              modeRef.current === "dictate"
                ? (base64) => {
                    if (partialSessionRef.current !== session) return;
                    sttQueueRef.current.push(base64);
                    void drainSttQueue(session);
                  }
                : undefined,
          });
          openingRef.current = false;
          const pending = pendingActionRef.current;
          pendingActionRef.current = null;
          if (pending === "cancel") {
            await cancelCurrentRecording();
            return;
          }
          if (await soundsEnabled()) playStartPing();
          await emit("engine-status", "recording");
          if (pending === "stop") {
            await finishRecording(modeRef.current);
          }
        } catch (err) {
          const pending = pendingActionRef.current;
          openingRef.current = false;
          pendingActionRef.current = null;
          await resetNativeRecordingSession();
          if (pending === "cancel") {
            await emit("mic-level", 0);
            await emit("engine-status", "idle");
            return;
          }
          const message = `Microphone access failed: ${err}`;
          await logDictationError(message);
          await emit("dictation-error", message);
          await emit("engine-status", "error");
          await invoke("hide_bubble_window").catch(() => undefined);
        }
      });

      // Text-transform chords or Flow Bar Cancel: drop the recorder without processing.
      unlistenCancel = await listen<string>("recording-cancel", async () => {
        if (openingRef.current) {
          pendingActionRef.current = "cancel";
          await cancelRecording();
          return;
        }
        await cancelCurrentRecording();
      });

      unlistenStop = await listen<string>("recording-stop", async (event) => {
        const mode = parseMode(event.payload || modeRef.current);
        modeRef.current = mode;
        if (openingRef.current) {
          pendingActionRef.current = "stop";
          return;
        }
        await finishRecording(mode);
      });
    })();

    return () => {
      unlistenStart?.();
      unlistenStop?.();
      unlistenMode?.();
      unlistenCancel?.();
    };
  }, [enabled]);
}
