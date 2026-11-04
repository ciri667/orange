import { useMemo, useState } from "react";
import {
  AlertCircle,
  Bell,
  CalendarClock,
  CalendarDays,
  CalendarRange,
  CheckCircle2,
  ListFilter,
  LoaderCircle,
  MessageSquarePlus,
  PauseCircle,
  Plus,
  Repeat,
  Search,
  X,
} from "lucide-react";
import { Button } from "../shared/Button";
import { ListRow } from "../shared/ListRow";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { cn } from "../shared/cn";
import { focusShellClassName } from "../shared/ui";
import type { ScheduledJob, TaskKind } from "../shared/types";
import { TaskMonthDialog } from "./TaskMonthDialog";
import type { ReminderNotice } from "./reminderInbox";
import {
  formatTaskTime,
  groupTasks,
  taskAttentionCount,
  taskKind,
  taskKindLabel,
  taskLastResultLabel,
  taskStatus,
  taskStatusLabel,
  type TaskFilter,
} from "./taskUtils";

/** 左侧任务面板：只负责列表和新建入口，详情在右侧页签打开。 */
export function TaskPanel({
  tasks,
  activeTaskId,
  isLoading,
  error,
  isBusy,
  onCreateTask,
  onCreateTaskFromSession,
  onOpenTask,
  onRetry,
  reminderNotices = [],
  onDismissReminder,
  onClearReminders,
  compactPreview = false,
}: {
  tasks: ScheduledJob[];
  activeTaskId?: string;
  isLoading: boolean;
  error: string;
  isBusy: boolean;
  onCreateTask: (kind?: TaskKind) => void;
  onCreateTaskFromSession?: () => void;
  onOpenTask: (taskId: string) => void;
  onRetry: () => void;
  /** 未读提醒：面板顶部单独一条，避免只靠 Toast 一闪而过。 */
  reminderNotices?: ReminderNotice[];
  onDismissReminder?: (id: string) => void;
  onClearReminders?: () => void;
  compactPreview?: boolean;
}) {
  const [filter, setFilter] = useState<TaskFilter>("all");
  const [filterOpen, setFilterOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [monthOpen, setMonthOpen] = useState(false);

  const groups = useMemo(() => groupTasks(tasks, filter, query), [tasks, filter, query]);
  const attention = taskAttentionCount(tasks);
  const filterLabel = filter === "all" ? "全部" : filter === "agenda" ? "日程" : "定时";

  return (
    <section
      className={cn("flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden", compactPreview && "bg-transparent")}
      aria-label="任务"
    >
      <header className="flex shrink-0 items-center gap-1.5 px-2.5 pb-1.5 pt-1">
        <h2 className="m-0 text-[15px] font-semibold text-ink-strong">任务</h2>
        {attention ? (
          <span className="inline-flex min-w-4 items-center justify-center rounded-full bg-accent-soft px-1 text-[10px] font-medium leading-4 text-accent">
            {attention}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-0.5">
          <Button
            variant="icon"
            size="compact"
            title="月历"
            aria-label="月历"
            onClick={() => setMonthOpen(true)}
          >
            <CalendarRange size={14} />
          </Button>
          {onCreateTaskFromSession ? (
            <Button
              variant="icon"
              size="compact"
              title="从当前会话创建任务"
              aria-label="从当前会话创建任务"
              onClick={onCreateTaskFromSession}
            >
              <MessageSquarePlus size={14} />
            </Button>
          ) : null}
          <Menu open={filterOpen} onClose={() => setFilterOpen(false)}>
            <Button
              variant="icon"
              size="compact"
              title={`筛选：${filterLabel}`}
              aria-label={`筛选：${filterLabel}`}
              aria-haspopup="menu"
              aria-expanded={filterOpen}
              onClick={() => setFilterOpen((open) => !open)}
            >
              <ListFilter size={14} />
            </Button>
            {filterOpen ? (
              <MenuPanel placement="bottom-end" className="left-auto right-0 min-w-[132px]">
                {(
                  [
                    ["all", "全部"],
                    ["agenda", "日程"],
                    ["scheduled", "定时任务"],
                  ] as const
                ).map(([value, label]) => (
                  <MenuItem
                    key={value}
                    onClick={() => {
                      setFilter(value);
                      setFilterOpen(false);
                    }}
                  >
                    <span className="flex-1">{label}</span>
                    {filter === value ? <span className="text-accent">✓</span> : null}
                  </MenuItem>
                ))}
              </MenuPanel>
            ) : null}
          </Menu>
          <Button
            variant="icon"
            size="compact"
            title="搜索任务"
            aria-label="搜索任务"
            aria-pressed={searchOpen}
            onClick={() => setSearchOpen((open) => !open)}
          >
            <Search size={14} />
          </Button>
        </div>
      </header>

      {reminderNotices.length ? (
        <section
          className="mx-1.5 mb-1.5 grid shrink-0 gap-1 rounded-control border border-accent/25 bg-accent-soft px-2 py-1.5"
          aria-label="未读提醒"
        >
          <div className="flex items-center gap-1.5">
            <Bell size={12} className="shrink-0 text-accent" aria-hidden />
            <span className="text-[11px] font-medium text-accent">{reminderNotices.length} 条未读提醒</span>
            {onClearReminders ? (
              <Button
                variant="text"
                size="compact"
                className="ml-auto min-h-5 px-1 text-[11px] text-accent"
                onClick={onClearReminders}
              >
                全部已读
              </Button>
            ) : null}
          </div>
          <ul className="m-0 grid max-h-32 list-none gap-0.5 overflow-auto p-0">
            {reminderNotices.map((notice) => (
              <li key={notice.id} className="flex items-start gap-1">
                <button
                  type="button"
                  className="grid min-w-0 flex-1 gap-0 rounded-control px-1 py-0.5 text-left hover:bg-surface/70"
                  onClick={() => onOpenTask(notice.jobId)}
                >
                  <span className="truncate text-[11px] font-medium text-ink">{notice.jobName}</span>
                  <span className="truncate text-[10px] text-ink-muted">{notice.preview}</span>
                </button>
                {onDismissReminder ? (
                  <Button
                    variant="icon"
                    size="compact"
                    className="size-5 min-h-5 text-ink-soft"
                    title="标记已读"
                    aria-label={`把「${notice.jobName}」的提醒标记为已读`}
                    onClick={() => onDismissReminder(notice.id)}
                  >
                    <X size={11} />
                  </Button>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <div className="shrink-0 px-1.5">
        <Button
          variant="ghost"
          className="w-full justify-start gap-2 px-2 text-ui"
          onClick={() => onCreateTask()}
          disabled={isBusy}
        >
          <Plus size={16} />
          新建任务
        </Button>
        {searchOpen ? (
          <label
            className={cn(
              "sidebar-search mb-1.5 flex min-h-8 items-center gap-2 rounded-control px-2 text-ink-muted",
              focusShellClassName,
            )}
          >
            <Search size={14} className="shrink-0" />
            <input
              className="min-w-0 w-full border-0 bg-transparent text-ui outline-0"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索任务"
              type="search"
              autoFocus
            />
          </label>
        ) : null}
      </div>

      <div className="min-h-0 flex-1 overflow-auto px-1.5 pb-3">
        {isLoading ? (
          <p className="m-0 px-2 py-3 text-xs text-ink-muted">正在读取任务…</p>
        ) : error ? (
          <div className="grid gap-1.5 px-2 py-3">
            <p className="m-0 text-xs text-danger">{error}</p>
            <Button size="compact" className="w-fit" onClick={onRetry}>
              重试
            </Button>
          </div>
        ) : !tasks.length ? (
          <div className="grid place-items-center gap-2 px-4 py-10 text-center">
            <CalendarClock size={28} className="text-ink-soft" aria-hidden />
            <p className="m-0 text-sm font-medium text-ink">暂无已安排的任务</p>
            <p className="m-0 text-xs leading-normal text-ink-muted">
              点击「新建任务」，把要做的事放到时间上。
            </p>
          </div>
        ) : !groups.length ? (
          <p className="m-0 px-2 py-3 text-xs text-ink-muted">没有匹配的任务。</p>
        ) : (
          <div className="grid gap-3">
            {groups.map((group) => (
              <section key={group.key} className="grid gap-0.5" aria-label={group.label}>
                <p
                  className={cn(
                    "m-0 px-2 pb-0.5 text-[11px] font-medium uppercase tracking-wide text-ink-soft",
                    group.tone === "danger" && "text-danger",
                  )}
                >
                  {group.label}
                  <span className="ml-1 font-normal normal-case text-ink-soft">{group.items.length}</span>
                </p>
                {group.items.map((task) => {
                  const status = taskStatus(task);
                  const kind = taskKind(task);
                  const lastResult = taskLastResultLabel(task);
                  return (
                    <ListRow
                      key={task.id}
                      active={task.id === activeTaskId}
                      className="items-start gap-2 py-1.5"
                      onClick={() => onOpenTask(task.id)}
                    >
                      <span className="w-10 shrink-0 pt-0.5 text-[11px] tabular-nums text-ink-muted">
                        {formatTaskTime(task)}
                      </span>
                      <span className="grid min-w-0 flex-1 gap-0.5">
                        <span className="flex min-w-0 items-center gap-1.5">
                          <span className="min-w-0 truncate text-[13px] font-medium text-ink-strong">
                            {task.name}
                          </span>
                          <span className="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-surface-muted px-1.5 py-0.5 text-[10px] text-ink-muted">
                            {task.outputMode === "reminder" ? (
                              <Bell size={10} />
                            ) : kind === "agenda" ? (
                              <CalendarDays size={10} />
                            ) : (
                              <Repeat size={10} />
                            )}
                            {taskKindLabel(task)}
                          </span>
                        </span>
                        <span className="flex items-center gap-1 text-[11px] text-ink-muted">
                          <StatusDot status={status} />
                          {taskStatusLabel(status)}
                          {lastResult && status !== "done" && status !== "missed" ? (
                            <span className="text-ink-soft">· {lastResult}</span>
                          ) : null}
                        </span>
                      </span>

                    </ListRow>
                  );
                })}
              </section>
            ))}
          </div>
        )}
      </div>
      {monthOpen ? (
        <TaskMonthDialog tasks={tasks} onClose={() => setMonthOpen(false)} onOpenTask={onOpenTask} />
      ) : null}
    </section>
  );
}

function StatusDot({ status }: { status: ReturnType<typeof taskStatus> }) {
  if (status === "running") {
    return <LoaderCircle size={11} className="animate-spin text-accent" />;
  }
  if (status === "done" || status === "reminded") {
    return <CheckCircle2 size={11} className="text-success" />;
  }
  if (status === "missed" || status === "failed") {
    return <AlertCircle size={11} className="text-danger" />;
  }
  if (status === "paused") {
    return <PauseCircle size={11} className="text-ink-soft" />;
  }
  return <span className="size-1.5 rounded-full bg-accent" aria-hidden />;
}
