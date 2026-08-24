import { Save, Sparkles } from "lucide-react";
import { Button } from "../../shared/Button";
import { settingsSectionClassName } from "../../shared/ui";
import { SettingsSectionHeader } from "../SettingsChrome";
import type { AgentSkill, UserSettings } from "../../shared/types";

/** Skills 设置摘要分区，完整 CRUD 仍由 SkillsModal 承载。 */
export function SkillsSettingsSection({
  skills,
  enabledSkillCount,
  customSkillCount,
  isBusy,
  onOpenSkillsModal,
  onSaveSettings,
}: {
  skills: AgentSkill[];
  enabledSkillCount: number;
  customSkillCount: number;
  isBusy: boolean;
  onOpenSkillsModal: () => void;
  onSaveSettings: () => void | Promise<void>;
}) {
  return (
    <section className={settingsSectionClassName} aria-labelledby="skills-settings-title">
      <SettingsSectionHeader
        kicker="Configuration"
        title="Skills 能力"
        titleId="skills-settings-title"
        description="管理 Agent 可用能力和未显式选择时的匹配方式。"
        actions={
          <>
            <Button variant="ghost" onClick={onOpenSkillsModal}>
              <Sparkles size={14} />
              管理 Skills
            </Button>
            <Button variant="primary" size="compact" onClick={onSaveSettings} disabled={isBusy}>
              <Save size={14} />
              保存设置
            </Button>
          </>
        }
      />
      <div className="grid grid-cols-3 gap-2 max-[820px]:grid-cols-1">
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">启用</span>
          <strong className="mt-1 block text-base text-ink-strong">
            {enabledSkillCount} / {skills.length}
          </strong>
        </div>
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">Prompt 注入</span>
          <strong className="mt-1 block text-base text-ink-strong">{enabledSkillCount} 个</strong>
        </div>
        <div className="rounded-control border border-border-translucent bg-warm-panel p-2.5">
          <span className="block text-xs text-ink-muted">自定义 Skills</span>
          <strong className="mt-1 block text-base text-ink-strong">{customSkillCount} 个</strong>
        </div>
      </div>
    </section>
  );
}
