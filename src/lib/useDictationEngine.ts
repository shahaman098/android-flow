import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { startRecording, stopRecording } from "./audio";

type Mode = "vibe" | "vibe_refine" | "dictate" | "command" | "prompt";

function parseMode(payload: string): Mode {
  if (payload === "vibe") return "vibe";
  if (payload === "vibe_refine") return "vibe_refine";
  if (payload === "dictate") return "dictate";
  if (payload === "command") return "command";
  if (payload === "prompt") return "prompt";
  return "vibe";
}

/**
 * Runs in the Hub (main) window so dictation works even if the bubble
 * webview is still cold. Bubble is display-only.
 */
export function useDictationEngine(enabled: boolean) {
  const busyRef = useRef(false);
  const modeRef = useRef<Mode>("vibe");

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

      unlistenStart = await listen<string>("recording-start", async (event) => {
        if (busyRef.current) {
          // The previous session is still transcribing. Say so — silently dropping
          // the session made the hotkey look broken with no explanation.
          await emit(
            "dictation-error",
            "Still processing the previous dictation. Wait for it to finish, then try again.",
          );
          return;
        }
        modeRef.current = parseMode(event.payload);
        try {
          await startRecording();
          await emit("engine-status", "recording");
        } catch (err) {
          await emit("dictation-error", `Microphone access failed: ${err}`);
          await emit("engine-status", "error");
          await invoke("hide_bubble_window").catch(() => undefined);
        }
      });

      // fn+2 pressed after fn had already opened a mic session: drop the recorder and its
      // audio without running any of it through the processing pipeline.
      unlistenCancel = await listen<string>("recording-cancel", async () => {
        try {
          await stopRecording();
        } catch {
          /* recorder may already be closed */
        }
      });

      unlistenStop = await listen<string>("recording-stop", async (event) => {
        if (busyRef.current) return;
        busyRef.current = true;
        const mode = parseMode(event.payload || modeRef.current);
        try {
          const audio = await stopRecording();
          if (!audio && mode !== "prompt" && mode !== "vibe_refine") {
            await emit(
              "dictation-error",
              "No audio captured. Check microphone permission and try again.",
            );
            await emit("engine-status", "error");
            await invoke("hide_bubble_window");
            return;
          }
          await emit("engine-status", "transcribing");
          const text = await invoke<string>("process_dictation", {
            audioBase64: audio || "",
            mode,
          });
          await emit("partial-transcript", text);
          await emit("engine-status", "idle");
        } catch (err) {
          await emit("dictation-error", String(err));
          await emit("engine-status", "error");
          await invoke("hide_bubble_window").catch(() => undefined);
        } finally {
          busyRef.current = false;
        }
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
