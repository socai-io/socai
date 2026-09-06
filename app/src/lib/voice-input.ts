import { invoke } from "@tauri-apps/api/core";

import { t } from "./i18n";

export type VoicePhase = "idle" | "requesting" | "recording" | "transcribing";
export type VoiceRoute = "cloud" | "local";

interface VoiceInputStatus {
  ready: boolean;
  route: VoiceRoute;
  state:
    | "cloud_ready"
    | "login_required"
    | "subscription_required"
    | "credits_required"
    | "billing_unavailable"
    | "local_only";
  local_state: "not_checked" | "ready" | "helper_missing" | "model_missing" | "downloading" | "error";
  downloaded_bytes: number;
  total_bytes: number;
  error: string | null;
}

export interface ComposerVoiceState {
  available: boolean;
  phase: VoicePhase;
  title: string;
  error: string;
}

interface VoiceTranscript {
  text: string;
}

interface LocalVoiceStatus {
  ready: boolean;
  state: VoiceInputStatus["local_state"];
  downloaded_bytes: number;
  total_bytes: number;
  error: string | null;
}

const TARGET_SAMPLE_RATE = 16_000;
const MAX_RECORDING_MS = 120_000;

export namespace voiceInput {
  let status: VoiceInputStatus | null = null;
  let phase: VoicePhase = "idle";
  let operationError = "";
  let statusUnavailable = false;
  let onChange: () => void = () => {};
  let refreshPromise: Promise<void> | null = null;

  let stream: MediaStream | null = null;
  let audioContext: AudioContext | null = null;
  let sourceNode: MediaStreamAudioSourceNode | null = null;
  let processorNode: ScriptProcessorNode | null = null;
  let silentGain: GainNode | null = null;
  let chunks: Float32Array[] = [];
  let capturedSampleRate = TARGET_SAMPLE_RATE;
  let recordingTimer: number | null = null;
  let localProgressTimer: number | null = null;
  let localProgressPromise: Promise<void> | null = null;
  let transcriptionRoute: VoiceRoute | null = null;
  let captureGeneration = 0;
  let automaticTranscriptHandler: ((text: string) => void) | null = null;

  export async function initialize(changeHandler: () => void): Promise<void> {
    onChange = changeHandler;
    await refresh(false);
  }

  export async function refresh(notify = true): Promise<void> {
    if (refreshPromise) return refreshPromise;
    refreshPromise = refreshStatus(notify).finally(() => {
      refreshPromise = null;
    });
    return refreshPromise;
  }

  export function isBusy(): boolean {
    return phase !== "idle";
  }

  export function composerState(): ComposerVoiceState {
    const mediaSupported = !!navigator.mediaDevices?.getUserMedia && typeof AudioContext !== "undefined";
    const available = !!status?.ready && mediaSupported;
    const activeRoute = transcriptionRoute ?? status?.route;
    let title: string;
    if (phase === "recording") title = t("voice.stop");
    else if (phase === "requesting") title = t("voice.requesting");
    else if (phase === "transcribing") {
      const transcribing = t(activeRoute === "local" ? "voice.transcribingLocal" : "voice.transcribingCloud");
      title = activeRoute === "local" && status
        ? `${transcribing} ${localStatusText(status)}`.trim()
        : transcribing;
    }
    else if (!mediaSupported) title = t("voice.unavailable.browser");
    else if (!status && !statusUnavailable) title = t("voice.unavailable.checking");
    else if (!status) title = t("voice.unavailable.billing");
    else if (status.ready && status.route === "cloud") title = t("voice.startCloud");
    else if (status.ready) {
      title = `${t("voice.startLocal")} ${localStatusText(status)}`.trim();
    }
    else title = `${accountStatusText(status)} ${localStatusText(status)}`.trim();
    return { available, phase, title, error: operationError };
  }

  export function setTranscriptionRoute(route: VoiceRoute): void {
    if (phase !== "transcribing") return;
    transcriptionRoute = route;
    if (route === "local") startLocalProgressPolling();
    else stopLocalProgressPolling();
    onChange();
  }

  /** Start on the first click; stop, transcribe, and return text on the next. */
  export async function toggle(
    onAutomaticTranscript?: (text: string) => void,
  ): Promise<string | null> {
    if (phase === "recording") return stopAndTranscribe();
    if (phase !== "idle" || !composerState().available) return null;
    automaticTranscriptHandler = onAutomaticTranscript ?? null;
    await startRecording();
    return null;
  }

  export function cancelRecording(message = ""): void {
    if (phase !== "recording" && phase !== "requesting") return;
    captureGeneration += 1;
    automaticTranscriptHandler = null;
    cleanupRecording();
    phase = "idle";
    operationError = message;
    onChange();
  }

  async function refreshStatus(notify: boolean): Promise<void> {
    try {
      status = await invoke<VoiceInputStatus>("voice_input_status");
      statusUnavailable = false;
    } catch (error) {
      console.error("voice_input_status failed:", error);
      status = null;
      statusUnavailable = true;
    }
    if (notify) onChange();
  }

  async function startRecording(): Promise<void> {
    const generation = captureGeneration + 1;
    captureGeneration = generation;
    phase = "requesting";
    operationError = "";
    onChange();
    try {
      const capturedStream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
        video: false,
      });
      if (generation !== captureGeneration) {
        capturedStream.getTracks().forEach((track) => track.stop());
        return;
      }
      stream = capturedStream;
      audioContext = new AudioContext({ sampleRate: TARGET_SAMPLE_RATE });
      capturedSampleRate = audioContext.sampleRate;
      chunks = [];
      sourceNode = audioContext.createMediaStreamSource(stream);
      processorNode = audioContext.createScriptProcessor(4096, 1, 1);
      silentGain = audioContext.createGain();
      silentGain.gain.value = 0;
      processorNode.onaudioprocess = (event) => {
        chunks.push(new Float32Array(event.inputBuffer.getChannelData(0)));
        event.outputBuffer.getChannelData(0).fill(0);
      };
      sourceNode.connect(processorNode);
      processorNode.connect(silentGain);
      silentGain.connect(audioContext.destination);
      phase = "recording";
      recordingTimer = window.setTimeout(() => {
        const handler = automaticTranscriptHandler;
        void stopAndTranscribe().then((text) => {
          if (text && handler) handler(text);
        });
      }, MAX_RECORDING_MS);
    } catch (error) {
      console.error("microphone capture failed:", error);
      automaticTranscriptHandler = null;
      cleanupRecording();
      phase = "idle";
      operationError = microphoneError(error);
    }
    onChange();
  }

  async function stopAndTranscribe(): Promise<string | null> {
    captureGeneration += 1;
    transcriptionRoute = status?.route ?? null;
    phase = "transcribing";
    operationError = "";
    const samples = mergeChunks(chunks);
    const sourceRate = capturedSampleRate;
    automaticTranscriptHandler = null;
    cleanupRecording();
    onChange();

    try {
      if (samples.length < sourceRate / 4) {
        throw new Error(t("voice.error.tooShort"));
      }
      const mono16k = resampleMono(samples, sourceRate, TARGET_SAMPLE_RATE)
        .subarray(0, TARGET_SAMPLE_RATE * (MAX_RECORDING_MS / 1000));
      const wav = encodePcm16Wav(mono16k, TARGET_SAMPLE_RATE);
      const result = await invoke<VoiceTranscript>("voice_input_transcribe", {
        audioBase64: bytesToBase64(wav),
      });
      const text = result.text.trim();
      if (!text) throw new Error(t("voice.error.noSpeech"));
      void refresh();
      return text;
    } catch (error) {
      console.error("voice_input_transcribe failed:", error);
      operationError = error instanceof Error ? error.message : `${error}`;
      return null;
    } finally {
      stopLocalProgressPolling();
      transcriptionRoute = null;
      phase = "idle";
      onChange();
    }
  }

  function startLocalProgressPolling(): void {
    if (localProgressTimer !== null) return;
    void refreshLocalProgress();
    localProgressTimer = window.setInterval(() => void refreshLocalProgress(), 1_000);
  }

  function stopLocalProgressPolling(): void {
    if (localProgressTimer !== null) window.clearInterval(localProgressTimer);
    localProgressTimer = null;
  }

  async function refreshLocalProgress(): Promise<void> {
    if (localProgressPromise || transcriptionRoute !== "local") return;
    localProgressPromise = invoke<LocalVoiceStatus>("voice_input_local_status")
      .then((local) => {
        if (!status || transcriptionRoute !== "local") return;
        status = {
          ...status,
          route: "local",
          local_state: local.state,
          downloaded_bytes: local.downloaded_bytes,
          total_bytes: local.total_bytes,
          error: local.error,
        };
        onChange();
      })
      .catch((error) => console.error("voice_input_local_status failed:", error))
      .finally(() => {
        localProgressPromise = null;
      });
    return localProgressPromise;
  }

  function accountStatusText(value: VoiceInputStatus): string {
    switch (value.state) {
      case "login_required":
        return t("voice.unavailable.login");
      case "subscription_required":
        return t("voice.unavailable.subscription");
      case "credits_required":
        return t("voice.unavailable.credits");
      case "billing_unavailable":
        return t("voice.unavailable.billing");
      case "local_only":
        return t("voice.unavailable.localOnly");
      case "cloud_ready":
        return "";
    }
  }

  function localStatusText(value: VoiceInputStatus): string {
    const size = formatBytes(value.total_bytes);
    switch (value.local_state) {
      case "model_missing":
        return t("voice.local.modelMissing", { size });
      case "downloading":
        return t("voice.local.downloading", { percent: downloadPercent(value) });
      case "ready":
        return t("voice.local.ready");
      case "helper_missing":
        return t("voice.local.helperMissing");
      case "error":
        return t("voice.local.failed");
      case "not_checked":
        return "";
    }
  }

  function cleanupRecording(): void {
    if (recordingTimer !== null) window.clearTimeout(recordingTimer);
    recordingTimer = null;
    if (processorNode) {
      processorNode.onaudioprocess = null;
      processorNode.disconnect();
    }
    sourceNode?.disconnect();
    silentGain?.disconnect();
    stream?.getTracks().forEach((track) => track.stop());
    if (audioContext && audioContext.state !== "closed") void audioContext.close();
    stream = null;
    audioContext = null;
    sourceNode = null;
    processorNode = null;
    silentGain = null;
    chunks = [];
  }

  function microphoneError(error: unknown): string {
    const name = error instanceof DOMException ? error.name : "";
    if (name === "NotAllowedError" || name === "SecurityError") return t("voice.error.permission");
    if (name === "NotFoundError" || name === "DevicesNotFoundError") return t("voice.error.noDevice");
    return t("voice.error.capture");
  }
}

function downloadPercent(status: VoiceInputStatus): number {
  if (!status.total_bytes) return 0;
  return Math.min(100, Math.max(0, Math.round((status.downloaded_bytes / status.total_bytes) * 100)));
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "359 MiB";
  return `${Math.round(bytes / (1024 * 1024))} MiB`;
}

function mergeChunks(chunks: Float32Array[]): Float32Array {
  const length = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const merged = new Float32Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return merged;
}

function resampleMono(input: Float32Array, sourceRate: number, targetRate: number): Float32Array {
  if (sourceRate === targetRate) return input;
  const ratio = sourceRate / targetRate;
  const output = new Float32Array(Math.max(1, Math.floor(input.length / ratio)));
  for (let index = 0; index < output.length; index += 1) {
    const start = Math.floor(index * ratio);
    const end = Math.min(input.length, Math.max(start + 1, Math.floor((index + 1) * ratio)));
    let sum = 0;
    for (let cursor = start; cursor < end; cursor += 1) sum += input[cursor];
    output[index] = sum / (end - start);
  }
  return output;
}

function encodePcm16Wav(samples: Float32Array, sampleRate: number): Uint8Array {
  const dataBytes = samples.length * 2;
  const buffer = new ArrayBuffer(44 + dataBytes);
  const view = new DataView(buffer);
  writeAscii(view, 0, "RIFF");
  view.setUint32(4, 36 + dataBytes, true);
  writeAscii(view, 8, "WAVE");
  writeAscii(view, 12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeAscii(view, 36, "data");
  view.setUint32(40, dataBytes, true);
  for (let index = 0; index < samples.length; index += 1) {
    const value = Math.max(-1, Math.min(1, samples[index]));
    view.setInt16(44 + index * 2, value < 0 ? value * 0x8000 : value * 0x7fff, true);
  }
  return new Uint8Array(buffer);
}

function writeAscii(view: DataView, offset: number, value: string): void {
  for (let index = 0; index < value.length; index += 1) {
    view.setUint8(offset + index, value.charCodeAt(index));
  }
}

function bytesToBase64(bytes: Uint8Array): string {
  const blockSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += blockSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + blockSize));
  }
  return btoa(binary);
}
