import { useState } from "react";
import { Save } from "lucide-react";
import { AGENT_SECURITY_LEVEL_COPY } from "../../agent/AgentPanelSections";
import { Button } from "../../shared/Button";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { SegmentedControl, SegmentedControlItem } from "../../shared/SegmentedControl";
import { fieldControlClassName, fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import type { AgentSecurityLevel, UserSettings } from "../../shared/types";
import { SettingsSectionHeader, SettingsSubblockHeader } from "../SettingsChrome";

const DEFAULT_LEVELS = ["basic", "advanced", "autonomous"] as const satisfies readonly AgentSecurityLevel[];

/** 把默认档收成后端会接受的值；未知档按基础展示。 */
function selectedDefaultLevel(level: AgentSecurityLevel): AgentSecurityLevel {
  return level === "advanced" || level === "autonomous" ? level : "basic";
}

/** 选择更高默认档时打开对应能力开关，避免保存时被后端降回基础。改回基础不收回已授权的能力。 */
function withDefaultLevel(security: UserSettings["agentSecurity"], level: AgentSecurityLevel): UserSettings["agentSecurity"] {
  return {
    ...security,
    defaultLevel: level,
    advancedExecutionEnabled: level === "basic" ? security.advancedExecutionEnabled : true,
    autonomousModeEnabled: level === "autonomous" ? true : security.autonomousModeEnabled,
  };
}

/** Agent 权限分区设置新会话默认档，并保存隔离任务资源上限。 */
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
  const selectedLevel = selectedDefaultLevel(security.defaultLevel);
  const selectedCopy = AGENT_SECURITY_LEVEL_COPY[selectedLevel];
  const [confirmAutonomousDefault, setConfirmAutonomousDefault] = useState(false);

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

  /** 写入草稿中的默认档；完全权限要等确认弹窗点头后才进入草稿。 */
  function applyDefaultLevel(level: AgentSecurityLevel) {
    onSettingsChange({
      ...settingsDraft,
      agentSecurity: withDefaultLevel(security, level),
    });
  }

  function selectDefaultLevel(level: AgentSecurityLevel) {
    if (isBusy || level === selectedLevel) {
      return;
    }
    if (level === "autonomous") {
      setConfirmAutonomousDefault(true);
      return;
    }
    applyDefaultLevel(level);
  }

  return (
    <section className={settingsSectionClassName} aria-labelledby="agent-security-settings-title">
      <SettingsSectionHeader
        kicker="Security"
        title="Agent 权限"
        titleId="agent-security-settings-title"
        description="新会话按这里的默认权限开始。已经打开的会话保持原样，需要时在协作区里单独切换。"
        actions={
          <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
            <Save size={14} />
            保存设置
          </Button>
        }
      />

      <div className="grid gap-3">
        <SettingsSubblockHeader title="新会话默认权限" description="只作用于之后新建的本地会话。" />
        <div className="grid justify-items-start gap-2">
          <SegmentedControl aria-label="新会话默认权限" role="radiogroup">
            {DEFAULT_LEVELS.map((level) => {
              const copy = AGENT_SECURITY_LEVEL_COPY[level];
              const active = selectedLevel === level;
              return (
                <SegmentedControlItem
                  key={level}
                  active={active}
                  role="radio"
                  aria-checked={active}
                  title={copy.description}
                  disabled={isBusy}
                  onClick={() => selectDefaultLevel(level)}
                >
                  {copy.label}
                </SegmentedControlItem>
              );
            })}
          </SegmentedControl>
          <p className="m-0 text-xs leading-normal text-ink-muted">{selectedCopy.description}</p>
        </div>
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

      {confirmAutonomousDefault && (
        <ConfirmDialog
          title="新会话默认使用完全权限"
          message="之后新建的会话会以完全权限开始：校验通过的写入自动落盘，已授权的 Skill 可连续运行。已经打开的会话不会被改掉。这不是整台电脑，系统保护目录和隔离边界仍然生效。"
          confirmLabel="设为默认"
          tone="default"
          isBusy={isBusy}
          onCancel={() => setConfirmAutonomousDefault(false)}
          onConfirm={() => {
            applyDefaultLevel("autonomous");
            setConfirmAutonomousDefault(false);
          }}
        />
      )}
    </section>
  );
}
