/** Energy-based endpointing, not a speech recognizer. Uses captured sample time,
 * so background-tab timer throttling cannot submit mid-sentence. */
export class VoiceActivityDetector {
  private elapsedMs = 0;
  private speechMs = 0;
  private quietMs = 0;
  private candidateMs = 0;
  private noise = 0.002;
  private level = 0;

  update(samples: Float32Array, rate: number): { level: number; stop: "silence" | "empty" | null } {
    if (!Number.isFinite(rate) || rate <= 0 || !samples.length) return { level: this.level, stop: null };
    let sum = 0, square = 0;
    for (const raw of samples) {
      const value = Number.isFinite(raw) ? raw : 0;
      sum += value; square += value * value;
    }
    const energy = Math.sqrt(Math.max(0, square / samples.length - (sum / samples.length) ** 2));
    const ms = samples.length / rate * 1000;
    this.elapsedMs += ms;
    const voiced = energy > Math.max(0.008, Math.min(0.035, this.noise * 3));
    if (voiced) {
      this.candidateMs += ms;
      if (this.candidateMs >= 200) this.speechMs += ms;
      this.quietMs = 0;
    } else {
      this.noise = this.noise * 0.95 + energy * 0.05;
      this.candidateMs = 0;
      this.quietMs += ms;
    }
    this.level = Math.min(1, Math.max(0, Math.sqrt(energy) * 2.5));
    const stop = this.speechMs > 0 && this.quietMs >= 1500 ? "silence"
      : this.speechMs === 0 && this.elapsedMs >= 12000 ? "empty" : null;
    return { level: this.level, stop };
  }
}

/** Update just the meter; never rerender or replace the user's draft on audio frames. */
export function paintVoiceActivity(level: number): void {
  document.querySelectorAll<HTMLElement>("[data-voice-meter]").forEach(node => {
    node.style.setProperty("--voice-level", String(Math.min(1, Math.max(0, level))));
  });
}
