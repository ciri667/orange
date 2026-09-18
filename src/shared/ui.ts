import { cn } from "./cn";

/** 区块小标题，如 知识库 / 文件 / 设置。 */
export const sectionLabelClassName =
  "m-0 px-2.5 text-[11px] font-medium tracking-[0.02em] text-ink-soft";

/** 路径小标题，颜色比 section-label 稍实。 */
export const pathLabelClassName =
  "m-0 truncate text-[11px] font-medium tracking-[0.01em] text-ink-muted";

/** 设置 / 表单字段标签。 */
export const fieldLabelClassName = "grid min-w-0 gap-2 text-xs font-bold text-ink-muted";

/** 文本输入的共用控件外观。 */
export const fieldControlClassName =
  "w-full min-w-0 min-h-[var(--control-height)] rounded-control border border-border bg-control px-2.5 font-normal text-ink";

/** 多行输入。 */
export const fieldTextareaClassName = cn(fieldControlClassName, "min-h-24 resize-y py-2.5 leading-[1.55]");

/** 设置页卡片。 */
export const settingsCardClassName =
  "grid gap-2.5 rounded-xl border border-border bg-surface p-3";

/** 设置分区根。 */
export const settingsSectionClassName = "grid min-w-0 content-start gap-4";

/** 设置分区标题行。 */
export const settingsContentTitleClassName =
  "flex items-start justify-between gap-3 border-b border-border pb-4 max-[820px]:grid";

/** Agent 会话 / 范围 / 上下文浮层。 */
export const agentPopoverClassName =
  "absolute inset-3 z-agent-popover grid grid-rows-[auto_minmax(0,1fr)] overflow-hidden rounded-2xl border border-border bg-surface p-3.5 shadow-app";

/** 浮层标题行。 */
export const popoverHeaderClassName = "flex items-center justify-between gap-3";
