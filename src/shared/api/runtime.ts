import { invoke, isTauri } from "@tauri-apps/api/core";
import { logDebug, logError } from "../logger";

/** 带脱敏日志的 Tauri invoke 包装，只记录命令名、状态和耗时，不记录 payload。 */
export async function invokeLogged<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const startedAt = performance.now();

  logDebug("调用 Tauri 命令。", {
    category: "frontend",
    event: "tauri_invoke",
    command,
    status: "started",
  });

  try {
    const result = await invoke<T>(command, args);

    logDebug("Tauri 命令完成。", {
      category: "frontend",
      event: "tauri_invoke",
      command,
      status: "completed",
      durationMs: performance.now() - startedAt,
    });

    return result;
  } catch (error) {
    logError("Tauri 命令失败。", {
      category: "frontend",
      event: "tauri_invoke",
      command,
      status: "failed",
      durationMs: performance.now() - startedAt,
      error,
    });

    throw error;
  }
}

declare global {
  interface Window {
    /** Tauri v2 运行时标记，官方 isTauri helper 会优先读取这个值。 */
    isTauri?: boolean;
    /** Tauri 运行时注入对象，用于区分桌面环境与浏览器开发环境。 */
    __TAURI_INTERNALS__?: unknown;
  }
}

/** 判断当前是否运行在 Tauri 桌面壳中。 */
export function isTauriRuntime() {
  return isTauri() || (typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__));
}
