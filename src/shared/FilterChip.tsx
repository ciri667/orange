import { type ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

/**
 * 可切换的筛选芯片。
 * 用于来源 / 标签等互斥筛选，选中态不是主行动按钮。
 */
export function FilterChip({
  active = false,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { active?: boolean }) {
  return (
    <button
      type={type}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full border border-border-translucent bg-surface-translucent px-2 py-[5px] text-xs text-ink-muted",
        active && "border-primary-border-strong bg-accent-soft text-accent-strong",
        className,
      )}
      {...props}
    />
  );
}
