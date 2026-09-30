import { listen } from "@tauri-apps/api/event";
import { invokeLogged, isTauriRuntime } from "./runtime";

/** 本机中文语音模型状态。不包含路径、音频或识别正文。 */
export interface VoiceModelStatus {
  supported: boolean;
  ready: boolean;
  downloading: boolean;
  receivedBytes: number;
  totalBytes: number | null;
  message: string;
}

/** 一次按住说话的识别结果。 */
export interface VoiceTranscript {
  text: string;
  durationMs: number;
  truncated: boolean;
}

const BROWSER_STATUS: VoiceModelStatus = {
  supported: false,
  ready: false,
  downloading: false,
  receivedBytes: 0,
  totalBytes: null,
  message: "浏览器开发态不支持语音输入，请使用桌面端。",
};

/** 读取语音模型是否已下载。浏览器开发态固定为不可用。 */
export async function loadVoiceModelStatus(): Promise<VoiceModelStatus> {
  if (!isTauriRuntime()) {
    return BROWSER_STATUS;
  }

  return invokeLogged<VoiceModelStatus>("load_voice_model_status");
}

/** 下载 SenseVoice 模型。进度回调只收到字节数和说明。 */
export async function downloadVoiceModel(onProgress?: (status: VoiceModelStatus) => void): Promise<VoiceModelStatus> {
  if (!isTauriRuntime()) {
    throw new Error(BROWSER_STATUS.message);
  }

  const unlisten = await listen<VoiceModelStatus>("voice-model-progress", (event) => {
    onProgress?.(event.payload);
  });
  try {
    return await invokeLogged<VoiceModelStatus>("download_voice_model");
  } finally {
    unlisten();
  }
}

/** 删除已下载的语音模型。 */
export async function deleteVoiceModel(): Promise<VoiceModelStatus> {
  if (!isTauriRuntime()) {
    throw new Error(BROWSER_STATUS.message);
  }

  return invokeLogged<VoiceModelStatus>("delete_voice_model");
}

/** 按住按钮时开始采集麦克风。 */
export async function startVoiceCapture(): Promise<void> {
  if (!isTauriRuntime()) {
    throw new Error(BROWSER_STATUS.message);
  }

  return invokeLogged<void>("start_voice_capture");
}

/** 松开按钮后识别，并把文本交回输入框。 */
export async function stopVoiceCapture(): Promise<VoiceTranscript> {
  if (!isTauriRuntime()) {
    throw new Error(BROWSER_STATUS.message);
  }

  return invokeLogged<VoiceTranscript>("stop_voice_capture");
}
