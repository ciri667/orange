import { type ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

/** 选中态列表行的视觉参数。 */
export interface ListRowVisualProps {
  active?: boolean;
  error?: boolean;
  className?: string;
}

/** 知识库行、会话行、Skill 行等选中态列表的共用 class。 */
export function listRowClassName({ active = false, error = false, className }: ListRowVisualProps = {}) {
  return cn(
    "flex w-full min-w-0 items-center gap-2.5 rounded-control border border-transparent bg-transparent p-2.5 text-left text-ink",
    "hover:bg-surface-hover",
    active && "border-primary-border-strong bg-accent-soft text-accent-strong",
    error && "border-[rgba(180,35,24,0.28)] bg-danger-soft text-danger",
    className,
  );
}

/**
 * 可点击的选中态列表行。
 * 不是主行动按钮；label / div 容器请用 listRowClassName。
 */
export function ListRow({
  active = false,
  error = false,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & ListRowVisualProps) {
  return <button type={type} className={listRowClassName({ active, error, className })} {...props} />;
}
