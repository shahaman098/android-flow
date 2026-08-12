// Mic-side capture for meeting transcription. Audio never touches disk — each ~20s window is
// recorded by its own fresh MediaRecorder instance (stop-and-restart on the same MediaStream, so
// the mic permission is only requested once) and handed off as an independently valid WebM/Opus
// blob, rather than re-encoding the whole call on every chunk the way the dictation partial-
// transcript preview does.

const CHUNK_MS = 20000;

let stream: MediaStream | null = null;
let recorder: MediaRecorder | null = null;
let restartTimer: number | undefined;
let mimeType: string | undefined;

function pickMimeType(): string | undefined {
  const candidates = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4"];
  return candidates.find((type) => MediaRecorder.isTypeSupported(type));
}

async function blobToBase64(blob: Blob): Promise<string> {
  const buffer = await blob.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

function startChunkRecorder(onChunk: (base64: string) => void) {
  if (!stream) return;
  const chunks: BlobPart[] = [];
  const rec = mimeType
    ? new MediaRecorder(stream, { mimeType })
    : new MediaRecorder(stream);

  rec.ondataavailable = (event) => {
    if (event.data.size > 0) chunks.push(event.data);
  };
  rec.onstop = () => {
    if (chunks.length === 0) return;
    const blob = new Blob(chunks, { type: rec.mimeType || "audio/webm" });
    if (blob.size < 2000) return;
    void blobToBase64(blob).then(onChunk);
  };

  rec.start();
  recorder = rec;
}

export async function startMeetingMicCapture(
  onChunk: (base64: string) => void,
): Promise<void> {
  if (stream) return; // already running

  stream = await navigator.mediaDevices.getUserMedia({
    audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
  });
  mimeType = pickMimeType();
  startChunkRecorder(onChunk);

  restartTimer = window.setInterval(() => {
    const current = recorder;
    if (!current || current.state !== "recording") return;
    current.stop(); // fires onstop -> onChunk, then we start a fresh window
    startChunkRecorder(onChunk);
  }, CHUNK_MS);
}

export async function stopMeetingMicCapture(): Promise<void> {
  if (restartTimer) {
    window.clearInterval(restartTimer);
    restartTimer = undefined;
  }

  const current = recorder;
  recorder = null;
  if (current && current.state === "recording") {
    await new Promise<void>((resolve) => {
      // Chain onto the onstop set by startChunkRecorder (which flushes `chunks` into
      // onChunk) rather than replacing it, so the final partial window isn't dropped.
      const originalOnStop = current.onstop;
      current.onstop = (ev) => {
        if (typeof originalOnStop === "function") originalOnStop.call(current, ev);
        resolve();
      };
      current.stop();
    });
  }

  stream?.getTracks().forEach((track) => track.stop());
  stream = null;
}
