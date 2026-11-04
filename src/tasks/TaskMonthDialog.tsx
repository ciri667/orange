import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "../shared/Button";
import { ModalBackdrop, ModalPanel } from "../shared/Modal";
import { cn } from "../shared/cn";
import type { ScheduledJob } from "../shared/types";
import { formatTaskTime, taskStatusLabel, taskStatus, taskWhen } from "./taskUtils";

const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"];

function dayKey(date: Date): string {
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

function startOfMonth(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

/** 月历总览：按天聚合任务，点某天看当天清单；不改变侧栏的分组逻辑。 */
export function TaskMonthDialog({
  tasks,
  onClose,
  onOpenTask,
}: {
  tasks: ScheduledJob[];
  onClose: () => void;
  onOpenTask: (taskId: string) => void;
}) {
  const [month, setMonth] = useState(() => startOfMonth(new Date()));
  const [selectedKey, setSelectedKey] = useState(() => dayKey(new Date()));

  function shiftMonth(delta: number) {
    const next = startOfMonth(new Date(month.getFullYear(), month.getMonth() + delta, 1));
    setMonth(next);
    setSelectedKey(dayKey(next));
  }

  const tasksByDay = useMemo(() => {
    const map = new Map<string, ScheduledJob[]>();
    for (const task of tasks) {
      const when = taskWhen(task);
      if (!when) {
        continue;
      }
      const key = dayKey(when);
      const list = map.get(key) ?? [];
      list.push(task);
      map.set(key, list);
    }
    return map;
  }, [tasks]);

  const first = startOfMonth(month);
  const offset = (first.getDay() + 6) % 7;
  const daysInMonth = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
  const cells = Array.from({ length: offset + daysInMonth }, (_, index) => {
    if (index < offset) {
      return null;
    }
    return new Date(month.getFullYear(), month.getMonth(), index - offset + 1);
  });
  const selectedTasks = tasksByDay.get(selectedKey) ?? [];

  return createPortal(
    <ModalBackdrop onClose={onClose}>
      <ModalPanel
        aria-label="任务月历"
        className="grid max-h-[min(760px,calc(100vh-32px))] w-[min(640px,calc(100vw-40px))] grid-rows-[auto_minmax(0,1fr)]"
      >
        <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
          <div>
            <h2 className="m-0 text-base font-semibold text-ink-strong">任务月历</h2>
            <p className="m-0 text-xs text-ink-muted">
              {month.getFullYear()} 年 {month.getMonth() + 1} 月
            </p>
          </div>
          <div className="flex items-center gap-1">
            <Button
              variant="icon"
              size="compact"
              title="上个月"
              aria-label="上个月"
              onClick={() => shiftMonth(-1)}
            >
              <ChevronLeft size={15} />
            </Button>
            <Button
              size="compact"
              onClick={() => {
                const today = new Date();
                setMonth(startOfMonth(today));
                setSelectedKey(dayKey(today));
              }}
            >
              今天
            </Button>
            <Button
              variant="icon"
              size="compact"
              title="下个月"
              aria-label="下个月"
              onClick={() => shiftMonth(1)}
            >
              <ChevronRight size={15} />
            </Button>
          </div>
        </header>
        <div className="grid min-h-0 grid-rows-[auto_auto_minmax(0,1fr)] gap-3 overflow-hidden p-4">
          <div className="grid grid-cols-7 gap-1 text-center text-[11px] text-ink-soft">
            {WEEKDAYS.map((weekday) => (
              <span key={weekday}>{weekday}</span>
            ))}
          </div>
          <div className="grid grid-cols-7 gap-1">
            {cells.map((date, index) => {
              if (!date) {
                return <span key={`empty-${index}`} />;
              }
              const key = dayKey(date);
              const dayTasks = tasksByDay.get(key) ?? [];
              const isToday = key === dayKey(new Date());
              const selected = key === selectedKey;
              return (
                <button
                  key={key}
                  type="button"
                  className={cn(
                    "grid min-h-12 content-start gap-0.5 rounded-control border bg-transparent p-1 text-left text-[11px] transition-colors",
                    selected
                      ? "border-border-strong bg-surface-muted text-ink-strong"
                      : "border-transparent text-ink hover:border-border hover:bg-surface-hover",
                    isToday && !selected && "border-accent/40 text-accent",
                  )}
                  onClick={() => setSelectedKey(key)}
                >
                  <span className="font-medium">{date.getDate()}</span>
                  <span className="flex gap-0.5">
                    {dayTasks.slice(0, 3).map((task) => (
                      <span
                        key={task.id}
                        className={cn(
                          "size-1.5 rounded-full",
                          taskStatus(task) === "missed" || taskStatus(task) === "failed"
                            ? "bg-danger"
                            : taskStatus(task) === "done"
                              ? "bg-ink-soft"
                              : "bg-accent",
                        )}
                      />
                    ))}
                    {dayTasks.length > 3 ? <span className="text-[9px] text-ink-soft">+{dayTasks.length - 3}</span> : null}
                  </span>
                </button>
              );
            })}
          </div>
          <div className="min-h-0 overflow-auto border-t border-border pt-3">
            <p className="m-0 pb-1 text-xs font-medium text-ink-muted">
              {selectedKey === dayKey(new Date()) ? "今天" : "所选日期"} · {selectedTasks.length} 个任务
            </p>
            {selectedTasks.length ? (
              <ul className="m-0 grid list-none gap-1 p-0">
                {selectedTasks.map((task) => (
                  <li key={task.id}>
                    <button
                      type="button"
                      className="flex w-full items-center gap-2 rounded-control border-0 bg-transparent px-2 py-1.5 text-left hover:bg-surface-hover"
                      onClick={() => {
                        onOpenTask(task.id);
                        onClose();
                      }}
                    >
                      <span className="w-14 shrink-0 text-[11px] tabular-nums text-ink-muted">
                        {formatTaskTime(task)}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-xs text-ink-strong">{task.name}</span>
                      <span className="text-[11px] text-ink-muted">{taskStatusLabel(taskStatus(task))}</span>
                    </button>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="m-0 py-2 text-xs text-ink-soft">这天没有任务。</p>
            )}
          </div>
        </div>
      </ModalPanel>
    </ModalBackdrop>,
    document.body,
  );
}
