import { Loader2 } from "lucide-react";
import { cn } from "./cn";

/** 操作结果提示：侧栏和空知识库启动页共用，错误态按文案关键字上色。 */
export function OperationNotice({
  notice,
  busyLabel,
  isBusy = false,
  className,
}: {
  notice: string;
  busyLabel?: string;
  isBusy?: boolean;
  className?: string;
}) {
  if (!isBusy && !notice) {
    return null;
  }

  const isError = notice.includes("失败") || notice.includes("阻止");

  return (
    <div
      className={cn(
        "flex items-center gap-2 rounded-control border border-primary-border bg-accent-soft px-2.5 py-[9px] text-left text-xs leading-[1.45] text-accent-strong",
        isError && "border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft text-danger",
        className,
      )}
      role="status"
    >
      {isBusy && <Loader2 size={14} className="shrink-0 animate-spin" />}
      <span>{busyLabel || notice}</span>
    </div>
  );
}
