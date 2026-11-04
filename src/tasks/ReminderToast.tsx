import { Bell, X } from "lucide-react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { formatRunTime } from "./taskUtils";
import type { ReminderNotice } from "./reminderInbox";

/**
 * 应用内提醒 Toast。
 *
 * 只在窗口前台时才有意义——窗口不在前台时后端会同时发系统通知，
 * 所以这里不做「窗口是否可见」的判断，避免两处逻辑打架。
 * Toast 会自动收起，未读仍然留在任务面板的提醒条里。
 */
export function ReminderToastStack({
  toasts,
  onOpen,
  onDismiss,
}: {
  toasts: ReminderNotice[];
  onOpen: (notice: ReminderNotice) => void;
  onDismiss: (id: string) => void;
}) {
  if (!toasts.length) {
    return null;
  }

  return (
    <div className="reminder-toast-stack" role="region" aria-label="提醒">
      {toasts.map((notice) => (
        <div
          key={notice.id}
          className={cn(
            "pointer-events-auto grid gap-2 rounded-panel border border-border bg-surface px-3.5 py-3 shadow-[0_18px_44px_rgba(16,24,32,0.16)]",
          )}
          role="status"
        >
          <div className="flex items-start gap-2">
            <span className="mt-0.5 inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
              <Bell size={13} aria-hidden />
            </span>
            <div className="grid min-w-0 flex-1 gap-0.5">
              <span className="truncate text-sm font-medium text-ink-strong">{notice.jobName}</span>
              <span className="text-[11px] text-ink-muted">{formatRunTime(notice.firedAt)}</span>
            </div>
            <Button
              variant="icon"
              size="compact"
              title="关闭提醒"
              aria-label="关闭提醒"
              onClick={() => onDismiss(notice.id)}
            >
              <X size={13} />
            </Button>
          </div>
          <p className="m-0 line-clamp-2 text-xs leading-relaxed text-ink-muted">{notice.preview}</p>
          <div className="flex justify-end gap-1">
            <Button size="compact" variant="text" onClick={() => onDismiss(notice.id)}>
              知道了
            </Button>
            <Button size="compact" variant="primary" onClick={() => onOpen(notice)}>
              查看任务
            </Button>
          </div>
        </div>
      ))}
    </div>
  );
}
