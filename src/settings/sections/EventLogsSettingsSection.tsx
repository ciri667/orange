import { FolderOpen, RotateCw, Trash2 } from "lucide-react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { SelectControl } from "../../shared/SelectControl";
import { fieldLabelClassName, settingsCardClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import type { AppEventLog, AppEventLogCategory, AppEventLogLevel } from "../../shared/types";

/** 运行日志分区，支持级别/分类筛选、刷新和清空。 */
export function EventLogsSettingsSection({
  appEventLogs,
  eventLogLevel,
  eventLogCategory,
  isBusy,
  onEventLogLevelChange,
  onEventLogCategoryChange,
  onRefreshAppEventLogs,
  onClearAppEventLogs,
  onOpenAppLogFolder,
}: {
  appEventLogs: AppEventLog[];
  eventLogLevel: AppEventLogLevel | "";
  eventLogCategory: AppEventLogCategory | "";
  isBusy: boolean;
  onEventLogLevelChange: (level: AppEventLogLevel | "") => void;
  onEventLogCategoryChange: (category: AppEventLogCategory | "") => void;
  onRefreshAppEventLogs: () => void | Promise<void>;
  onClearAppEventLogs: () => void | Promise<void>;
  onOpenAppLogFolder: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="event-log-settings-title">
      <SettingsSectionHeader
        kicker="Diagnostics"
        title="运行日志"
        titleId="event-log-settings-title"
        description="查看应用事件日志，按级别和分类筛选。"
        actions={
          <>
            <Button variant="ghost" onClick={onOpenAppLogFolder} disabled={isBusy}>
              <FolderOpen size={14} />
              文件日志
            </Button>
            <Button variant="ghost" onClick={onRefreshAppEventLogs} disabled={isBusy}>
              <RotateCw size={14} />
              刷新
            </Button>
            <Button variant="ghost" tone="danger" onClick={onClearAppEventLogs} disabled={isBusy}>
              <Trash2 size={14} />
              清空
            </Button>
          </>
        }
      />
      <div className="grid grid-cols-2 gap-2.5 max-[820px]:grid-cols-1">
        <label className={fieldLabelClassName}>
          <span>级别</span>
          <SelectControl value={eventLogLevel} onChange={(event) => onEventLogLevelChange(event.target.value as AppEventLogLevel | "")}>
            <option value="">全部</option>
            <option value="error">错误</option>
            <option value="warn">警告</option>
            <option value="info">信息</option>
            <option value="debug">调试</option>
          </SelectControl>
        </label>
        <label className={fieldLabelClassName}>
          <span>分类</span>
          <SelectControl value={eventLogCategory} onChange={(event) => onEventLogCategoryChange(event.target.value as AppEventLogCategory | "")}>
            <option value="">全部</option>
            <option value="app">应用</option>
            <option value="storage">存储</option>
            <option value="knowledge_base">知识库</option>
            <option value="editor">编辑器</option>
            <option value="agent">Agent</option>
            <option value="im">即时通讯</option>
            <option value="model">模型</option>
            <option value="skill">Skill</option>
            <option value="settings">设置</option>
            <option value="security">安全</option>
            <option value="frontend">前端</option>
          </SelectControl>
        </label>
      </div>
      <div className="grid gap-2.5">
        {appEventLogs.length ? appEventLogs.map((log) => <AppEventLogCard key={log.id} log={log} />) : <p className="m-0 text-[13px] text-ink-muted">暂无运行日志。</p>}
      </div>
    </section>
  );
}

/** 单条应用事件日志卡片，展示运行级别、分类、状态和脱敏上下文。 */
function AppEventLogCard({ log }: { log: AppEventLog }) {
  return (
    <article
      className={cn(
        settingsCardClassName,
        "gap-1.5 p-2.5",
        log.level === "error" && "border-[rgba(var(--danger-rgb),0.28)] bg-danger-soft",
        log.level === "warn" && "border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft",
        log.level === "debug" && "bg-surface-muted",
      )}
    >
      <div className="flex min-w-0 items-center justify-between gap-2 text-[13px]">
        <OverflowTooltipText
          as="strong"
          className="min-w-0 truncate"
          text={`${formatEventLogLevel(log.level)} · ${formatEventLogCategory(log.category)}`}
          logArea="settings_event_log_kind"
        />
        <OverflowTooltipText className="min-w-0 truncate text-ink-muted" text={log.createdAt} logArea="settings_event_log_created_at" />
      </div>
      <OverflowTooltipText as="p" className="m-0 text-ink" text={`${formatEventStatus(log.status)} / ${log.event}`} logArea="settings_event_log_status" />
      <p className="m-0 text-ink">{log.message}</p>
      <OverflowTooltipText as="code" className="min-w-0 [overflow-wrap:anywhere] font-mono text-ink-muted [word-break:break-word]" text={formatEventLogContext(log)} logArea="settings_event_log_context" />
    </article>
  );
}

/** 把后端审计类型转成简短中文标签。 */
function formatAuditKind(kind: string) {
  const labels: Record<string, string> = {
    model_turn: "模型请求",
    model_error_turn: "模型失败",
    local_rule_turn: "本地规则",
    browser_mock_turn: "浏览器模拟",
  };

  return labels[kind] ?? kind;
}

/** 把运行日志级别转成设置页中文标签。 */
function formatEventLogLevel(level: AppEventLogLevel) {
  const labels: Record<AppEventLogLevel, string> = {
    debug: "调试",
    info: "信息",
    warn: "警告",
    error: "错误",
  };

  return labels[level];
}

/** 把运行日志分类转成设置页中文标签。 */
function formatEventLogCategory(category: AppEventLogCategory) {
  const labels: Record<AppEventLogCategory, string> = {
    app: "应用",
    storage: "存储",
    knowledge_base: "知识库",
    editor: "编辑器",
    agent: "Agent",
    im: "即时通讯",
    model: "模型",
    skill: "Skill",
    settings: "设置",
    security: "安全",
    frontend: "前端",
  };

  return labels[category];
}

/** 把后端事件状态转成简短中文标签，保留未知状态原文便于排查。 */
function formatEventStatus(status: string) {
  const labels: Record<string, string> = {
    started: "开始",
    completed: "完成",
    failed: "失败",
    blocked: "阻止",
  };

  return labels[status] ?? status;
}

/** 汇总事件日志的轻量上下文，避免卡片中散落过多字段。 */
function formatEventLogContext(log: AppEventLog) {
  const parts = [
    log.operationId ? `op=${log.operationId}` : "",
    log.sessionId ? `session=${log.sessionId}` : "",
    log.knowledgeBaseId ? `kb=${log.knowledgeBaseId}` : "",
    log.entityType && log.entityId ? `${log.entityType}=${log.entityId}` : "",
    log.relativePath ? `path=${log.relativePath}` : "",
    typeof log.durationMs === "number" ? `${log.durationMs}ms` : "",
  ].filter(Boolean);

  return parts.length ? parts.join(" · ") : "无额外上下文";
}
