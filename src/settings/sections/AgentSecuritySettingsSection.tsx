import { useState } from "react";
import { AGENT_SECURITY_LEVEL_COPY } from "../../agent/AgentPanelSections";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import { SegmentedControl, SegmentedControlItem } from "../../shared/SegmentedControl";
import { fieldControlClassName, fieldLabelClassName, settingsSectionClassName } from "../../shared/ui";
import type { AgentSecurityLevel, UserSettings } from "../../shared/types";
import { SettingsSectionHeader, SettingsSubblockHeader } from "../SettingsChrome";
import type { SettingsDraftChange } from "../useSettingsAutosave";

const DEFAULT_LEVELS = ["workspace-readonly", "workspace-write", "full-confirm", "full-auto"] as const satisfies readonly AgentSecurityLevel[];

/** 把默认档收成后端会接受的值；未知档按工作区只读展示。 */
function selectedDefaultLevel(level: AgentSecurityLevel): AgentSecurityLevel {
  return DEFAULT_LEVELS.includes(level) ? level : "workspace-readonly";
}

/** 默认档只影响之后新建的本地会话，不联动旧执行开关。 */
function withDefaultLevel(security: UserSettings["agentSecurity"], level: AgentSecurityLevel): UserSettings["agentSecurity"] {
  return {
    ...security,
    defaultLevel: level,

  };
}

/** Agent 权限分区设置新会话默认档，并保存真实命令超时。 */
export function AgentSecuritySettingsSection({
  settingsDraft,
  isBusy,
  onSettingsChange,
}: {
  settingsDraft: UserSettings;
  isBusy: boolean;
  onSettingsChange: SettingsDraftChange<UserSettings>;
}) {
  const security = settingsDraft.agentSecurity;
  const selectedLevel = selectedDefaultLevel(security.defaultLevel);
  const selectedCopy = AGENT_SECURITY_LEVEL_COPY[selectedLevel];
  const [confirmDefault, setConfirmDefault] = useState<AgentSecurityLevel>();

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
    }, true);
  }

  /** 默认档选择后自动保存；完全权限仍须先完成确认。 */
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
    if (level === "full-auto" || level === "full-confirm") {
      setConfirmDefault(level);
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
        <SettingsSubblockHeader title="命令超时" description="真实命令使用受管理进程；输出总捕获上限为 1 MiB。此设置不是内存、进程数量配额或文件系统隔离。" />
        <div className="grid grid-cols-2 gap-x-3.5 gap-y-3 max-[820px]:grid-cols-1">
          <label className={fieldLabelClassName}><span>超时（秒）</span><input className={fieldControlClassName} type="number" min={5} max={1800} value={security.resourceLimits.timeoutSeconds} onChange={(event) => updateLimit("timeoutSeconds", Number(event.target.value))} /></label>
        </div>
      </div>

      {confirmDefault && (
        <ConfirmDialog
          title={`新会话默认使用${AGENT_SECURITY_LEVEL_COPY[confirmDefault].label}`}
          message={`新会话可访问工作区外资源、外部服务并在当前系统用户的真实环境执行命令；不会自动提权，不提供文件系统沙箱隔离。${confirmDefault === "full-auto" ? "此档免去逐次审批并自动应用新变更。" : "外部读取和操作逐次审批。"}已有会话保持原权限。命令失败或取消也可能已有副作用。`}
          confirmLabel="设为默认"
          tone="default"
          isBusy={isBusy}
          onCancel={() => setConfirmDefault(undefined)}
          onConfirm={() => {
            applyDefaultLevel(confirmDefault);
            setConfirmDefault(undefined);
          }}
        />
      )}
    </section>
  );
}
