import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { startMeetingMicCapture, stopMeetingMicCapture } from "./meetingAudio";
import type { MeetingStatus } from "./types";

/**
 * Owns the mic-chunk restart loop for meeting transcription. Purely reactive to
 * `meeting-phase-changed` — the backend's consent gate is what actually allows capture to start;
 * this hook just starts/stops the mic pipeline in lockstep with the backend's reported phase.
 */
export function useMeetingCapture() {
  const capturingRef = useRef(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    (async () => {
      unlisten = await listen<MeetingStatus>("meeting-phase-changed", (event) => {
        const capturing = event.payload.phase === "capturing";

        if (capturing && !capturingRef.current) {
          capturingRef.current = true;
          void startMeetingMicCapture((audioBase64) => {
            void invoke("meeting_submit_mic_chunk", { audioBase64 }).catch(() => undefined);
          }).catch(() => {
            capturingRef.current = false;
          });
        } else if (!capturing && capturingRef.current) {
          capturingRef.current = false;
          void stopMeetingMicCapture();
        }
      });
    })();

    return () => {
      unlisten?.();
      if (capturingRef.current) {
        capturingRef.current = false;
        void stopMeetingMicCapture();
      }
    };
  }, []);
}
