import { X } from "lucide-react";
import { type ReactNode } from "react";
import { cn } from "./cn";

/**
 * 可移除标签。
 * 用于本轮 Skill、@ 文件等压缩展示，不是主行动按钮。
 */
export function Chip({
  children,
  missing = false,
  onRemove,
  removeLabel,
  className,
}: {
  children: ReactNode;
  missing?: boolean;
  onRemove?: () => void;
  removeLabel?: string;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex min-w-0 max-w-[150px] items-center gap-1.5 overflow-hidden rounded-full border border-border bg-surface py-0.5 pr-1.5 pl-2 text-xs font-medium text-ink-muted",
        missing && "border-warning/35 bg-warning/10 text-warning",
        className,
      )}
    >
      {children}
      {onRemove ? (
        <button
          type="button"
          className="inline-grid size-[18px] shrink-0 place-items-center rounded-full border-0 bg-transparent text-inherit hover:bg-surface-hover"
          aria-label={removeLabel}
          onClick={onRemove}
        >
          <X size={12} />
        </button>
      ) : null}
    </span>
  );
}
