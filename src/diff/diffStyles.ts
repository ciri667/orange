import { cn } from "../shared/cn";

/** 统一 diff 文件头。 */
export const unifiedDiffFileClassName =
  "sticky top-0 z-[1] flex items-center justify-between gap-3 border-b border-border bg-warm-panel px-2.5 py-2 text-xs font-extrabold text-ink";

/** hunk 头。 */
export const diffHunkHeaderClassName =
  "flex w-full items-center gap-2 border-0 bg-agent-soft px-2.5 py-[7px] text-left font-mono text-xs text-agent-strong";

/** 单行 diff 的网格骨架。 */
export const diffLineGridClassName =
  "grid w-full min-w-0 grid-cols-[44px_44px_22px_minmax(0,1fr)_auto] items-stretch border-l-[3px] border-transparent p-0 font-mono text-xs leading-[1.55] max-[820px]:grid-cols-[34px_34px_18px_minmax(0,1fr)_auto] max-[820px]:text-[11px]";

/** 按增删上下文给 diff 行上色。 */
export function diffLineToneClassName(kind: "added" | "removed" | "context" | "placeholder") {
  return cn(
    kind === "added" && "bg-success-soft [&_.diff-line-marker]:text-success [&_.diff-line-number-new]:text-success",
    kind === "removed" && "bg-danger-soft [&_.diff-line-marker]:text-danger [&_.diff-line-number-old]:text-danger",
    kind === "placeholder" && "bg-warm-panel text-ink-muted",
    kind === "context" && "bg-surface text-ink",
  );
}

/** 行号 / 标记列。 */
export const diffGutterClassName =
  "inline-flex min-h-6 items-center justify-end px-[7px] text-ink-soft select-none";
