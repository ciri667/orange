import { type ButtonHTMLAttributes, type ReactNode } from "react";
import { cn } from "./cn";

/**
 * 互斥分段开关的轨道。
 * 不能套用 <Button>：选中态是轨道内高亮，不是主行动点。
 */
export function SegmentedControl({
  children,
  className,
  "aria-label": ariaLabel,
  role = "group",
}: {
  children: ReactNode;
  className?: string;
  "aria-label": string;
  role?: "group" | "radiogroup";
}) {
  return (
    <div
      className={cn(
        "inline-flex items-center rounded-control border border-border bg-surface p-0.5",
        className,
      )}
      role={role}
      aria-label={ariaLabel}
    >
      {children}
    </div>
  );
}

/** 分段开关中的一项。active 表示当前选中。 */
export function SegmentedControlItem({
  active = false,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { active?: boolean }) {
  return (
    <button
      type={type}
      className={cn(
        "inline-flex min-h-7 items-center justify-center gap-1.5 rounded-small border-0 bg-transparent px-2.5 text-xs text-ink-muted",
        "hover:enabled:bg-surface hover:enabled:text-ink",
        active && "bg-surface font-medium text-ink-strong shadow-[inset_0_0_0_1px_var(--border-strong)]",
        className,
      )}
      {...props}
    />
  );
}
