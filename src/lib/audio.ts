let mediaRecorder: MediaRecorder | null = null;
let chunks: BlobPart[] = [];
let activeStream: MediaStream | null = null;

function pickMimeType(): string | undefined {
  const candidates = [
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/mp4",
  ];
  return candidates.find((type) => MediaRecorder.isTypeSupported(type));
}

export async function startRecording(): Promise<void> {
  if (mediaRecorder?.state === "recording") return;

  activeStream = await navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });

  chunks = [];
  const mimeType = pickMimeType();
  mediaRecorder = mimeType
    ? new MediaRecorder(activeStream, { mimeType })
    : new MediaRecorder(activeStream);

  mediaRecorder.ondataavailable = (event) => {
    if (event.data.size > 0) chunks.push(event.data);
  };

  // A timeslice makes WebKit emit MP4/WebM fragments that are not guaranteed to
  // form a valid file when concatenated. Let stop() produce one finalized blob.
  mediaRecorder.start();
}

export async function stopRecording(): Promise<string | null> {
  const recorder = mediaRecorder;
  if (!recorder || recorder.state === "inactive") {
    cleanupStream();
    return null;
  }

  const blob = await new Promise<Blob | null>((resolve) => {
    recorder.onstop = () => {
      const type = recorder.mimeType || "audio/webm";
      resolve(chunks.length ? new Blob(chunks, { type }) : null);
    };
    recorder.stop();
  });

  cleanupStream();
  mediaRecorder = null;
  chunks = [];

  if (!blob || blob.size === 0) return null;
  return blobToBase64(blob);
}

function cleanupStream() {
  activeStream?.getTracks().forEach((track) => track.stop());
  activeStream = null;
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
