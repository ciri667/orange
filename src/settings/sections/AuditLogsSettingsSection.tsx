import { RotateCw } from "lucide-react";
import { Button } from "../../shared/Button";
import { cn } from "../../shared/cn";
import { OverflowTooltipText } from "../../shared/OverflowTooltipText";
import { settingsCardClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import type { RequestAuditLog } from "../../shared/types";

/** 请求审计分区，展示模型请求和工具边界摘要。 */
export function AuditLogsSettingsSection({
  auditLogs,
  isBusy,
  onRefreshAuditLogs,
}: {
  auditLogs: RequestAuditLog[];
  isBusy: boolean;
  onRefreshAuditLogs: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="audit-settings-title">
      <SettingsSectionHeader
        kicker="Diagnostics"
        title="请求审计"
        titleId="audit-settings-title"
        description="查看最近模型请求、本地规则回退和工具边界摘要。"
        actions={
          <Button variant="ghost" onClick={onRefreshAuditLogs} disabled={isBusy}>
            <RotateCw size={14} />
            刷新
          </Button>
        }
      />
      <div className="grid gap-2.5">
        {auditLogs.length ? auditLogs.map((log) => <AuditLogCard key={log.id} log={log} />) : <p className="m-0 text-[13px] text-ink-muted">暂无审计记录。</p>}
      </div>
    </section>
  );
}

/** 单条审计日志卡片，展示请求类型、scope 摘要和工具调用摘要。 */
function AuditLogCard({ log }: { log: RequestAuditLog }) {
  return (
    <article className={cn(settingsCardClassName, "gap-1.5 p-2.5")}>
      <div className="flex min-w-0 items-center justify-between gap-2 text-[13px]">
        <OverflowTooltipText as="strong" className="min-w-0 truncate" text={formatAuditKind(log.kind)} logArea="settings_audit_kind" />
        <OverflowTooltipText className="min-w-0 truncate text-ink-muted" text={log.createdAt} logArea="settings_audit_created_at" />
      </div>
      <p className="m-0 text-ink">{log.scopeSummary}</p>
      <p className="m-0 text-ink">{log.contentSummary}</p>
      <OverflowTooltipText as="code" className="min-w-0 [overflow-wrap:anywhere] font-mono text-ink-muted [word-break:break-word]" text={log.toolSummary} logArea="settings_audit_tool_summary" />
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
