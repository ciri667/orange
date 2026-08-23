import { type ReactNode } from "react";
import { Checkbox } from "./Checkbox";
import { cn } from "./cn";

/**
 * 开关行：左侧勾选框，右侧说明。
 * compact 用于卡片内短标签，例如 Provider「启用」。
 */
export function ToggleRow({
  checked,
  onChange,
  disabled,
  compact = false,
  children,
  className,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  compact?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <label
      className={cn(
        "relative grid items-center text-[13px] font-normal text-ink",
        compact ? "grid-cols-[18px_auto] gap-1.5 text-xs" : "grid-cols-[18px_minmax(0,1fr)] gap-2",
        className,
      )}
    >
      <Checkbox checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <span>{children}</span>
    </label>
  );
}
