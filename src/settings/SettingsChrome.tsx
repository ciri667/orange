import { type ReactNode } from "react";
import { cn } from "../shared/cn";
import { sectionLabelClassName, settingsContentTitleClassName } from "../shared/ui";

/** 设置分区标题：左侧说明，右侧操作。 */
export function SettingsSectionHeader({
  kicker,
  title,
  titleId,
  description,
  actions,
}: {
  kicker: string;
  title: string;
  titleId: string;
  description: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className={settingsContentTitleClassName}>
      <div className="min-w-0">
        <p className={sectionLabelClassName}>{kicker}</p>
        <h3 id={titleId} className="m-0 text-xl leading-tight text-ink-strong [overflow-wrap:anywhere]">
          {title}
        </h3>
        <p className="mt-[5px] mb-0 max-w-[680px] text-[13px] leading-[1.55] text-ink-muted">{description}</p>
      </div>
      {actions ? (
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-1.5 max-[820px]:justify-start">{actions}</div>
      ) : null}
    </div>
  );
}

/** 设置分区内的次级标题。 */
export function SettingsSubblockHeader({ title, description }: { title: string; description: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="min-w-0">
        <h4 className="m-0 text-[13px] font-bold text-ink-strong">{title}</h4>
        <p className="mt-[5px] mb-0 text-[13px] leading-[1.55] text-ink-muted">{description}</p>
      </div>
    </div>
  );
}

/** 设置页提示条。 */
export function SettingsPolicyRow({ icon, children, className }: { icon: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-control border border-border-translucent bg-warm-panel p-3 text-[13px] leading-[1.55] text-ink",
        className,
      )}
    >
      <span className="mt-0.5 shrink-0 text-accent">{icon}</span>
      <span>{children}</span>
    </div>
  );
}
