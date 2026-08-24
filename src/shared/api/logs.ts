import { invokeLogged, isTauriRuntime } from "./runtime";
import { createLocalId, formatLocalDateTime } from "../id";
import { browserMock } from "../mock/browser";
import {
  AppEventLog,
  AppEventLogCategory,
  AppEventLogLevel,
  RequestAuditLog,
} from "../types";

/** 读取最近请求审计日志，用于展示模型发送范围和工具调用摘要。 */
export async function loadRequestAuditLogs(): Promise<RequestAuditLog[]> {
  if (!isTauriRuntime()) {
    return browserMock.auditLogs;
  }

  return invokeLogged<RequestAuditLog[]>("load_request_audit_logs");
}

/** 读取最近应用事件日志，用于设置页展示运行诊断和关键操作。 */
export async function loadAppEventLogs(filters: {
  limit?: number;
  level?: AppEventLogLevel | "";
  category?: AppEventLogCategory | "";
} = {}): Promise<AppEventLog[]> {
  const payload = {
    limit: filters.limit ?? 100,
    level: filters.level || undefined,
    category: filters.category || undefined,
  };

  if (!isTauriRuntime()) {
    return browserMock.appEventLogs
      .filter((log) => !payload.level || log.level === payload.level)
      .filter((log) => !payload.category || log.category === payload.category)
      .slice(0, payload.limit);
  }

  return invokeLogged<AppEventLog[]>("load_app_event_logs", { payload });
}

/** 清空用户可读应用事件日志；桌面端不会删除文件诊断日志。 */
export async function clearAppEventLogs(): Promise<void> {
  if (!isTauriRuntime()) {
    browserMock.appEventLogs = [
      {
        id: createLocalId("event"),
        level: "info",
        category: "settings",
        event: "clear_app_event_logs",
        message: "已清空应用事件日志。",
        status: "completed",
        createdAt: formatLocalDateTime(),
      },
    ];
    return;
  }

  return invokeLogged<void>("clear_app_event_logs");
}

/** 打开 Tauri app log 目录，方便用户附带文件日志排查。 */
export async function openAppLogFolder(): Promise<string> {
  if (!isTauriRuntime()) {
    return "~/Library/Logs/app.orange.desktop";
  }

  return invokeLogged<string>("open_app_log_folder");
}
