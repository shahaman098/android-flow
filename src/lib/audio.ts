import { emit } from "@tauri-apps/api/event";

let mediaRecorder: MediaRecorder | null = null;
let chunks: BlobPart[] = [];
let activeStream: MediaStream | null = null;
let audioContext: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let levelTimer: number | null = null;
let startPromise: Promise<void> | null = null;
let sessionId = 0;
let partialRecorder: MediaRecorder | null = null;
let partialTimer: number | null = null;
let partialMimeType: string | undefined;
let partialChunkHandler: ((base64: string) => void) | null = null;
let pendingPartialFlushes: Promise<void>[] = [];
let sttRecorder: MediaRecorder | null = null;
let sttTimer: number | null = null;
let sttMimeType: string | undefined;
let sttChunkHandler: ((base64: string) => void) | null = null;
let pendingSttFlushes: Promise<void>[] = [];
const MICROPHONE_OPEN_TIMEOUT_MS = 8000;
const PARTIAL_CHUNK_MS = 2000;
const STT_CHUNK_MS = 50000;

type RecordingOptions = {
  onPartialChunk?: (base64: string) => void;
  onSttChunk?: (base64: string) => void;
};

type StopRecordingOptions = {
  flushPartial?: boolean;
};

function pickMimeType(): string | undefined {
  const candidates = [
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/mp4",
  ];
  return candidates.find((type) => MediaRecorder.isTypeSupported(type));
}

function stopLevelMeter() {
  if (levelTimer != null) {
    window.clearInterval(levelTimer);
    levelTimer = null;
  }
  try {
    void audioContext?.close();
  } catch {
    // already closed
  }
  audioContext = null;
  analyser = null;
  void emit("mic-level", 0);
}

function startLevelMeter(stream: MediaStream) {
  stopLevelMeter();
  try {
    const ctx = new AudioContext();
    const source = ctx.createMediaStreamSource(stream);
    const node = ctx.createAnalyser();
    node.fftSize = 256;
    node.smoothingTimeConstant = 0.55;
    source.connect(node);
    audioContext = ctx;
    analyser = node;
    const data = new Uint8Array(node.fftSize);

    levelTimer = window.setInterval(() => {
      if (!analyser) return;
      analyser.getByteTimeDomainData(data);
      let sum = 0;
      for (let i = 0; i < data.length; i += 1) {
        const v = (data[i] - 128) / 128;
        sum += v * v;
      }
      const rms = Math.sqrt(sum / data.length);
      // Boost quiet speech into a usable 0–1 visual range.
      const level = Math.min(1, Math.max(0, rms * 3.2));
      void emit("mic-level", level);
    }, 50);
  } catch {
    // Metering is best-effort; recording still works without it.
  }
}

function startPartialRecorder(stream: MediaStream) {
  if (!partialChunkHandler) return;
  const localChunks: BlobPart[] = [];
  const recorder = partialMimeType
    ? new MediaRecorder(stream, { mimeType: partialMimeType })
    : new MediaRecorder(stream);

  recorder.ondataavailable = (event) => {
    if (event.data.size > 0) localChunks.push(event.data);
  };

  recorder.onstop = () => {
    if (!partialChunkHandler || localChunks.length === 0) return;
    const blob = new Blob(localChunks, {
      type: recorder.mimeType || partialMimeType || "audio/webm",
    });
    if (blob.size < 2500) return;
    const handler = partialChunkHandler;
    const flush = blobToBase64(blob)
      .then((base64) => {
        handler?.(base64);
      })
      .catch(() => undefined);
    pendingPartialFlushes.push(flush);
    void flush.finally(() => {
      pendingPartialFlushes = pendingPartialFlushes.filter((item) => item !== flush);
    });
  };

  partialRecorder = recorder;
  recorder.start();
}

function startPartialLoop(stream: MediaStream, onChunk?: (base64: string) => void) {
  stopPartialLoop(false);
  if (!onChunk) return;

  partialChunkHandler = onChunk;
  partialMimeType = pickMimeType();
  startPartialRecorder(stream);
  partialTimer = window.setInterval(() => {
    const current = partialRecorder;
    if (!current || current.state !== "recording") return;
    current.stop();
    startPartialRecorder(stream);
  }, PARTIAL_CHUNK_MS);
}

function startSttRecorder(stream: MediaStream) {
  if (!sttChunkHandler) return;
  const localChunks: BlobPart[] = [];
  const recorder = sttMimeType
    ? new MediaRecorder(stream, { mimeType: sttMimeType })
    : new MediaRecorder(stream);

  recorder.ondataavailable = (event) => {
    if (event.data.size > 0) localChunks.push(event.data);
  };

  recorder.onstop = () => {
    if (!sttChunkHandler || localChunks.length === 0) return;
    const blob = new Blob(localChunks, {
      type: recorder.mimeType || sttMimeType || "audio/webm",
    });
    if (blob.size < 1500) return;
    const handler = sttChunkHandler;
    const flush = blobToBase64(blob)
      .then((base64) => {
        handler?.(base64);
      })
      .catch(() => undefined);
    pendingSttFlushes.push(flush);
    void flush.finally(() => {
      pendingSttFlushes = pendingSttFlushes.filter((item) => item !== flush);
    });
  };

  sttRecorder = recorder;
  recorder.start();
}

function startSttLoop(stream: MediaStream, onChunk?: (base64: string) => void) {
  stopSttLoop(false);
  if (!onChunk) return;

  sttChunkHandler = onChunk;
  sttMimeType = pickMimeType();
  startSttRecorder(stream);
  sttTimer = window.setInterval(() => {
    const current = sttRecorder;
    if (!current || current.state !== "recording") return;
    current.stop();
    startSttRecorder(stream);
  }, STT_CHUNK_MS);
}

function stopSttLoop(flushFinal: boolean) {
  if (sttTimer != null) {
    window.clearInterval(sttTimer);
    sttTimer = null;
  }

  const current = sttRecorder;
  sttRecorder = null;
  if (!flushFinal) {
    sttChunkHandler = null;
    sttMimeType = undefined;
    pendingSttFlushes = [];
  }
  if (current && current.state === "recording") {
    current.ondataavailable = flushFinal ? current.ondataavailable : null;
    current.onstop = flushFinal ? current.onstop : null;
    try {
      current.stop();
    } catch {
      // Recorder may already be stopping.
    }
  }
}

async function waitForSttFlushes(timeoutMs = 2000): Promise<void> {
  const pending = [...pendingSttFlushes];
  if (pending.length === 0) return;
  await Promise.race([
    Promise.allSettled(pending),
    new Promise((resolve) => window.setTimeout(resolve, timeoutMs)),
  ]);
}

function stopPartialLoop(flushFinal: boolean) {
  if (partialTimer != null) {
    window.clearInterval(partialTimer);
    partialTimer = null;
  }

  const current = partialRecorder;
  partialRecorder = null;
  if (!flushFinal) {
    partialChunkHandler = null;
    partialMimeType = undefined;
    pendingPartialFlushes = [];
  }
  if (current && current.state === "recording") {
    current.ondataavailable = flushFinal ? current.ondataavailable : null;
    current.onstop = flushFinal ? current.onstop : null;
    try {
      current.stop();
    } catch {
      // Recorder may already be stopping.
    }
  }
}

async function waitForPartialFlushes(timeoutMs = 1500): Promise<void> {
  const pending = [...pendingPartialFlushes];
  if (pending.length === 0) return;
  await Promise.race([
    Promise.allSettled(pending),
    new Promise((resolve) => window.setTimeout(resolve, timeoutMs)),
  ]);
}

function getUserMediaWithTimeout(
  constraints: MediaStreamConstraints,
  id: number,
): Promise<MediaStream> {
  let settled = false;
  let timeoutId: number | null = null;

  const mediaPromise = navigator.mediaDevices
    .getUserMedia(constraints)
    .then((stream) => {
      if (timeoutId != null) window.clearTimeout(timeoutId);
      if (settled || id !== sessionId) {
        stream.getTracks().forEach((track) => track.stop());
        throw new Error("Microphone request was cancelled.");
      }
      settled = true;
      return stream;
    })
    .catch((error) => {
      if (timeoutId != null) window.clearTimeout(timeoutId);
      if (!settled) settled = true;
      throw error;
    });

  const timeoutPromise = new Promise<MediaStream>((_, reject) => {
    timeoutId = window.setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(
        new Error(
          "Timed out waiting for microphone access. Check macOS Microphone permission for Flow, then try again.",
        ),
      );
    }, MICROPHONE_OPEN_TIMEOUT_MS);
  });

  return Promise.race([mediaPromise, timeoutPromise]);
}

export async function startRecording(options: RecordingOptions = {}): Promise<void> {
  if (mediaRecorder?.state === "recording") return;
  if (startPromise) return startPromise;

  const id = ++sessionId;
  startPromise = (async () => {
    let stream: MediaStream | null = null;
    try {
      stream = await getUserMediaWithTimeout({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      }, id);

      // A cancel may arrive while macOS is still resolving the microphone request.
      if (id !== sessionId) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }

      activeStream = stream;
      chunks = [];
      const mimeType = pickMimeType();
      const recorder = mimeType
        ? new MediaRecorder(stream, { mimeType })
        : new MediaRecorder(stream);
      mediaRecorder = recorder;

      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) chunks.push(event.data);
      };

      startLevelMeter(stream);
      startPartialLoop(stream, options.onPartialChunk);
      startSttLoop(stream, options.onSttChunk);

      // A timeslice makes WebKit emit fragments that are not guaranteed to form
      // one valid file. Let stop() produce a finalized recording.
      recorder.start();
    } catch (error) {
      stream?.getTracks().forEach((track) => track.stop());
      if (id === sessionId) cleanupRecorderState();
      throw error;
    } finally {
      if (id === sessionId) startPromise = null;
    }
  })();

  return startPromise;
}

export async function stopRecording(options: StopRecordingOptions = {}): Promise<string | null> {
  // A quick fn press/release can request stop before getUserMedia resolves.
  // Waiting here ensures that session is stopped, not silently left recording.
  if (startPromise) {
    try {
      await startPromise;
    } catch {
      return null;
    }
  }

  const recorder = mediaRecorder;
  if (!recorder || recorder.state === "inactive") {
    cleanupRecorderState();
    return null;
  }

  stopSttLoop(true);
  if (options.flushPartial) {
    stopPartialLoop(true);
  }

  const blob = await new Promise<Blob | null>((resolve) => {
    recorder.onstop = () => {
      const type = recorder.mimeType || "audio/webm";
      resolve(chunks.length ? new Blob(chunks, { type }) : null);
    };
    recorder.stop();
  });

  await waitForSttFlushes();
  sttChunkHandler = null;
  sttMimeType = undefined;
  if (options.flushPartial) {
    await waitForPartialFlushes();
    partialChunkHandler = null;
    partialMimeType = undefined;
  }

  cleanupRecorderState();

  if (!blob || blob.size === 0) return null;
  return blobToBase64(blob);
}

export async function cancelRecording(): Promise<void> {
  // Invalidate an in-flight getUserMedia request. If it resolves later, its
  // tracks are immediately closed instead of creating a ghost recording.
  sessionId += 1;
  const pending = startPromise;
  startPromise = null;
  try {
    await pending;
  } catch {
    // Permission/startup failure still ends in a clean idle state.
  }

  const recorder = mediaRecorder;
  if (recorder && recorder.state !== "inactive") {
    recorder.ondataavailable = null;
    recorder.onstop = null;
    try {
      recorder.stop();
    } catch {
      // Recorder may have transitioned between the state check and stop().
    }
  }
  cleanupRecorderState();
}

function cleanupRecorderState() {
  stopSttLoop(false);
  stopPartialLoop(false);
  stopLevelMeter();
  activeStream?.getTracks().forEach((track) => track.stop());
  activeStream = null;
  mediaRecorder = null;
  chunks = [];
}

async function blobToBase64(blob: Blob): Promise<string> {
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ""));
    reader.onerror = () =>
      reject(reader.error || new Error("Could not encode audio"));
    reader.readAsDataURL(blob);
  });
  const comma = dataUrl.indexOf(",");
  return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
}
