/** Soft start/stop cues via Web Audio — no asset files required. */

let sharedCtx: AudioContext | null = null;

function ctx(): AudioContext {
  if (!sharedCtx || sharedCtx.state === "closed") {
    sharedCtx = new AudioContext();
  }
  return sharedCtx;
}

function tone(
  frequency: number,
  durationMs: number,
  gainPeak: number,
  type: OscillatorType = "sine",
) {
  const audio = ctx();
  const osc = audio.createOscillator();
  const gain = audio.createGain();
  osc.type = type;
  osc.frequency.value = frequency;
  const now = audio.currentTime;
  gain.gain.setValueAtTime(0.0001, now);
  gain.gain.exponentialRampToValueAtTime(gainPeak, now + 0.012);
  gain.gain.exponentialRampToValueAtTime(0.0001, now + durationMs / 1000);
  osc.connect(gain);
  gain.connect(audio.destination);
  osc.start(now);
  osc.stop(now + durationMs / 1000 + 0.02);
}

/** Short confirmation that the mic is open. */
export function playStartPing() {
  try {
    void ctx().resume();
    tone(880, 70, 0.08);
    window.setTimeout(() => tone(1320, 55, 0.05), 45);
  } catch {
    // Sound is optional.
  }
}

/** Soft cue that capture ended. */
export function playStopSound() {
  try {
    void ctx().resume();
    tone(660, 60, 0.05);
    window.setTimeout(() => tone(440, 80, 0.04), 40);
  } catch {
    // Sound is optional.
  }
}

/** Brief cancel thump. */
export function playCancelSound() {
  try {
    void ctx().resume();
    tone(220, 90, 0.06, "triangle");
  } catch {
    // Sound is optional.
  }
}
