import { isTauri } from "@tauri-apps/api/core";
import { debug as tauriDebug, error as tauriError, info as tauriInfo, warn as tauriWarn } from "@tauri-apps/plugin-log";
import type { AppEventLogCategory, AppEventLogLevel } from "./types";

/** 前端日志上下文，只允许记录脱敏的命令名、事件名、状态和轻量计数。 */
export interface FrontendLogContext {
  category?: AppEventLogCategory;
  event?: string;
  command?: string;
  status?: string;
  durationMs?: number;
  error?: unknown;
  metadata?: Record<string, string | number | boolean | undefined>;
}

/** 记录调试日志；发布文件日志默认 Info 级别，Debug 主要服务开发态。 */
export function logDebug(message: string, context: FrontendLogContext = {}) {
  void writeFrontendLog("debug", message, context);
}

/** 记录普通运行日志，用于前端关键生命周期。 */
export function logInfo(message: string, context: FrontendLogContext = {}) {
  void writeFrontendLog("info", message, context);
}

/** 记录可恢复异常，例如浏览器 fallback 或非阻塞加载失败。 */
export function logWarn(message: string, context: FrontendLogContext = {}) {
  void writeFrontendLog("warn", message, context);
}

/** 记录前端错误，包含脱敏后的错误消息，不包含 invoke payload。 */
export function logError(message: string, context: FrontendLogContext = {}) {
  void writeFrontendLog("error", message, context);
}

/** 将未知错误转换为可写入日志的短文本，并移除常见敏感片段。 */
export function sanitizeErrorForLog(error: unknown) {
  if (error instanceof Error) {
    return sanitizeLogText(error.message);
  }

  return sanitizeLogText(String(error));
}

/** 统一写入前端日志，桌面端走 Tauri log plugin，浏览器开发态降级到 console。 */
async function writeFrontendLog(level: AppEventLogLevel, message: string, context: FrontendLogContext) {
  const sanitizedMessage = buildFrontendLogMessage(message, context);

  if (!isTauri()) {
    writeBrowserConsoleLog(level, sanitizedMessage);
    return;
  }

  try {
    const options = {
      keyValues: buildLogKeyValues(context),
    };

    if (level === "debug") {
      await tauriDebug(sanitizedMessage, options);
    } else if (level === "info") {
      await tauriInfo(sanitizedMessage, options);
    } else if (level === "warn") {
      await tauriWarn(sanitizedMessage, options);
    } else {
      await tauriError(sanitizedMessage, options);
    }
  } catch {
    // 日志写入失败不能影响业务路径；浏览器控制台也不再递归记录这个失败。
  }
}

/** 构造一行短日志文本，只包含固定上下文字段和脱敏错误。 */
function buildFrontendLogMessage(message: string, context: FrontendLogContext) {
  const fields = [
    `category=${context.category ?? "frontend"}`,
    context.event ? `event=${context.event}` : "",
    context.command ? `command=${context.command}` : "",
    context.status ? `status=${context.status}` : "",
    typeof context.durationMs === "number" ? `duration_ms=${Math.round(context.durationMs)}` : "",
    context.error ? `error=${sanitizeErrorForLog(context.error)}` : "",
  ].filter(Boolean);

  return sanitizeLogText([sanitizeLogText(message), ...fields].join(" "));
}

/** Tauri log plugin 的 keyValues 只能是字符串，这里显式收敛字段类型。 */
function buildLogKeyValues(context: FrontendLogContext) {
  const metadata = context.metadata ?? {};
  const keyValues: Record<string, string | undefined> = {
    category: context.category ?? "frontend",
    event: context.event,
    command: context.command,
    status: context.status,
  };

  for (const [key, value] of Object.entries(metadata)) {
    if (value === undefined) {
      continue;
    }

    keyValues[key] = sanitizeLogText(String(value));
  }

  return keyValues;
}

/** 浏览器开发态没有 Tauri 插件时使用 console，便于 Vite 中排查前端行为。 */
function writeBrowserConsoleLog(level: AppEventLogLevel, message: string) {
  const consoleMethod = level === "debug" ? "debug" : level === "info" ? "info" : level === "warn" ? "warn" : "error";

  globalThis.console?.[consoleMethod]?.(message);
}

/** 日志文本脱敏和截断，避免 API key、Authorization、绝对路径或超长正文进入诊断文件。 */
function sanitizeLogText(text: string) {
  const redacted = text
    .replace(/sk-[A-Za-z0-9_-]+/g, "[redacted]")
    .replace(/sess-[A-Za-z0-9_-]+/g, "[redacted]")
    .replace(/(api[_-]?key|authorization|bearer)\s*[:=]\s*[^,\s]+/gi, "$1=[redacted]")
    .replace(/(^|[\s"'`([{<])(?:~\/|\/(?:Users|Volumes|private|tmp|var|opt|home|Applications)\/)[^\s"'`)\]}>，。；;]+/g, "$1[path]")
    .replace(/(^|[\s"'`([{<])[A-Za-z]:[\\/][^\s"'`)\]}>，。；;]+/g, "$1[path]");
  const collapsed = redacted.replace(/\s+/g, " ").trim();

  return collapsed.length > 600 ? `${collapsed.slice(0, 600)}...` : collapsed;
}
