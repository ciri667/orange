import { Save, ShieldCheck } from "lucide-react";
import { Button } from "../../shared/Button";
import { ToggleRow } from "../../shared/ToggleRow";
import { fieldControlClassName, fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader, SettingsSubblockHeader } from "../SettingsChrome";
import type { UserSettings } from "../../shared/types";

/** Agent 权限分区解释三级放手策略，并保存隔离任务资源上限。 */
export function AgentSecuritySettingsSection({
  settingsDraft,
  isBusy,
  onSettingsChange,
  onSaveSettings,
}: {
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: (settings: UserSettings) => void;
  onSaveSettings: () => void | Promise<void>;
}) {
  const security = settingsDraft.agentSecurity;

  /** 数字输入在 UI 层限制到合理范围，后端保存时仍会二次归一化。 */
  function updateLimit(
    key: keyof UserSettings["agentSecurity"]["resourceLimits"],
    value: number,
  ) {
    onSettingsChange({
      ...settingsDraft,
      agentSecurity: {
        ...security,
        resourceLimits: {
          ...security.resourceLimits,
          [key]: Number.isFinite(value) ? value : 0,
        },
      },
    });
  }

  return (
    <section className={settingsSectionClassName} aria-labelledby="agent-security-settings-title">
      <SettingsSectionHeader
        kicker="Security"
        title="Agent 权限"
        titleId="agent-security-settings-title"
        description="三级权限决定你愿意把多少执行权交给 Agent，好让模型把能力用出来。Skill 只是更高权限下可发挥的能力之一。权限在 Agent 协作区按会话切换。"
        actions={
          <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </Button>
        }
      />

      <div className="grid gap-2" aria-label="三级权限说明">
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">基础</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">你盯紧每一步。Agent 只在知识库文档里工作，写入必须你确认。</p>
        </article>
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">进阶</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">开始放手。Agent 可以做更多事（整理目录、运行 Skill 等），落盘前仍要你看一眼。</p>
        </article>
        <article className="grid gap-1 rounded-lg border border-border bg-surface-warm px-3 py-2.5">
          <strong className="text-[13px] text-ink-strong">完全</strong>
          <p className="m-0 text-xs leading-normal text-ink-muted">真正放手。校验通过后连续执行并自动落盘，让模型一次把任务做完。系统保护边界仍然有效。</p>
        </article>
      </div>

      <div className="grid gap-3">
        <SettingsSubblockHeader title="隔离任务上限" description="当 Agent 被允许运行隔离进程时，超过任一上限就终止该任务。" />
        <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
          <label className={fieldLabelClassName}><span>超时（秒）</span><input className={fieldControlClassName} type="number" min={5} max={1800} value={security.resourceLimits.timeoutSeconds} onChange={(event) => updateLimit("timeoutSeconds", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>内存（MB）</span><input className={fieldControlClassName} type="number" min={64} max={4096} value={security.resourceLimits.maxMemoryMb} onChange={(event) => updateLimit("maxMemoryMb", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>进程数</span><input className={fieldControlClassName} type="number" min={1} max={64} value={security.resourceLimits.maxProcesses} onChange={(event) => updateLimit("maxProcesses", Number(event.target.value))} /></label>
          <label className={fieldLabelClassName}><span>产物（MB）</span><input className={fieldControlClassName} type="number" min={1} max={1024} value={security.resourceLimits.maxArtifactMb} onChange={(event) => updateLimit("maxArtifactMb", Number(event.target.value))} /></label>
        </div>
      </div>
    </section>
  );
}
