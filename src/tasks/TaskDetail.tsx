import { useEffect, useState } from "react";
import {
  AlertCircle,
  Bell,
  CalendarDays,
  CheckCircle2,
  Clock3,
  LoaderCircle,
  Pause,
  Pencil,
  Play,
  Repeat,
  Trash2,
} from "lucide-react";
import { Button } from "../shared/Button";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { cn } from "../shared/cn";
import { listScheduledJobRuns } from "../shared/tauriApi";
import type { KnowledgeBase, ScheduledJob, ScheduledJobRun, TaskStatus } from "../shared/types";
import {
  formatRunTime,
  formatTaskWhen,
  taskKind,
  taskKindLabel,
  taskLastResultLabel,
  taskStatus,
  taskStatusLabel,
} from "./taskUtils";

const STATUS_TONE: Record<TaskStatus, string> = {
  todo: "bg-accent-soft text-accent",
  scheduled: "bg-accent-soft text-accent",
  running: "bg-surface-muted text-ink",
  done: "bg-success-soft text-success",
  reminded: "bg-success-soft text-success",
  missed: "bg-danger-soft text-danger",
  paused: "bg-surface-muted text-ink-muted",
  failed: "bg-danger-soft text-danger",
};

/** 右侧任务详情页签：任务本身、操作和运行历史。 */
export function TaskDetail({
  task,
  knowledgeBases,
  isBusy,
  onEdit,
  onRun,
  onToggleEnabled,
  onDelete,
  onOpenSession,
}: {
  task: ScheduledJob;
  knowledgeBases: KnowledgeBase[];
  isBusy: boolean;
  onEdit: (task: ScheduledJob) => void;
  onRun: (taskId: string) => Promise<void>;
  onToggleEnabled: (task: ScheduledJob) => Promise<void>;
  onDelete: (taskId: string) => Promise<void>;
  onOpenSession: (sessionId: string) => void;
}) {
  const [runs, setRuns] = useState<ScheduledJobRun[]>([]);
  const [runsLoading, setRunsLoading] = useState(false);
  const [runsError, setRunsError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const status = taskStatus(task);
  const kind = taskKind(task);
  const scopeNames = task.knowledgeBaseIds
    .map((id) => knowledgeBases.find((knowledgeBase) => knowledgeBase.id === id)?.name)
    .filter((name): name is string => Boolean(name));

  useEffect(() => {
    let cancelled = false;
    setRunsLoading(true);
    setRunsError("");
    void listScheduledJobRuns(task.id)
      .then((next) => {
        if (!cancelled) {
          setRuns(next);
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setRunsError(error instanceof Error ? error.message : String(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setRunsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [task.id, task.lastRunAt, task.lastStatus]);

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="shrink-0 border-b border-border px-5 py-4">
        <div className="grid min-w-0 gap-2.5">
          <div className="grid min-w-0 gap-1.5">
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="inline-flex items-center gap-1 whitespace-nowrap rounded-full bg-surface-muted px-2 py-0.5 text-[11px] text-ink-muted">
                {task.outputMode === "reminder" ? (
                  <Bell size={11} />
                ) : kind === "agenda" ? (
                  <CalendarDays size={11} />
                ) : (
                  <Repeat size={11} />
                )}
                {taskKindLabel(task)}
              </span>
              <span className={cn("whitespace-nowrap rounded-full px-2 py-0.5 text-[11px] font-medium", STATUS_TONE[status])}>
                {taskStatusLabel(status)}
              </span>
              {taskLastResultLabel(task) && status !== "done" && status !== "missed" ? (
                <span className="whitespace-nowrap text-[11px] text-ink-soft">{taskLastResultLabel(task)}</span>
              ) : null}
            </div>
            <h1 className="m-0 truncate text-xl font-semibold tracking-tight text-ink-strong">{task.name}</h1>
            <p className="m-0 flex items-center gap-1.5 text-xs text-ink-muted">
              <Clock3 size={12} />
              {formatTaskWhen(task)}
              {kind === "scheduled" ? <span className="text-ink-soft">· {task.scheduleDisplay}</span> : null}
            </p>
          </div>
          <div className="flex max-w-full flex-wrap items-center gap-1">
            {status === "missed" || status === "failed" ? (
              <Button variant="primary" size="compact" disabled={isBusy} onClick={() => void onRun(task.id)}>
                <Play size={13} />
                补做
              </Button>
            ) : kind === "agenda" && status === "todo" ? (
              <>
                <Button variant="primary" size="compact" disabled={isBusy} onClick={() => onEdit(task)}>
                  <CalendarDays size={13} />
                  改期
                </Button>
                <Button size="compact" disabled={isBusy} onClick={() => void onRun(task.id)}>
                  <Play size={13} />
                  立即运行
                </Button>
              </>
            ) : (
              <Button size="compact" disabled={isBusy} onClick={() => void onRun(task.id)}>
                <Play size={13} />
                立即运行
              </Button>
            )}
            {kind === "scheduled" ? (
              <Button size="compact" disabled={isBusy} onClick={() => void onToggleEnabled(task)}>
                {task.enabled ? <Pause size={13} /> : <Play size={13} />}
                {task.enabled ? "暂停" : "恢复"}
              </Button>
            ) : null}
            <Button size="compact" disabled={isBusy} onClick={() => onEdit(task)}>
              <Pencil size={13} />
              编辑
            </Button>
            <Button
              variant="icon"
              tone="danger"
              size="compact"
              title="删除任务"
              aria-label="删除任务"
              disabled={isBusy}
              onClick={() => setConfirmDelete(true)}
            >
              <Trash2 size={14} />
            </Button>
          </div>
        </div>
      </header>

      <div className="grid min-h-0 flex-1 content-start gap-5 overflow-auto px-5 py-4">
        {status === "missed" ? (
          <div className="flex items-start gap-2 rounded-control border border-danger/30 bg-danger-soft px-3 py-2.5 text-sm text-danger">
            <AlertCircle size={16} className="mt-0.5 shrink-0" />
            <div className="grid gap-0.5">
              <span className="font-medium">这个日程已经错过</span>
              <span className="text-xs leading-normal">
                原定 {formatTaskWhen(task)}。系统不会自动补跑，点「补做」可以随时手动执行。
              </span>
            </div>
          </div>
        ) : null}

        {status === "failed" && task.lastError ? (
          <div className="flex items-start gap-2 rounded-control border border-danger/30 bg-danger-soft px-3 py-2.5 text-sm text-danger">
            <AlertCircle size={16} className="mt-0.5 shrink-0" />
            <span className="text-xs leading-normal">{task.lastError}</span>
          </div>
        ) : null}

        {status === "running" ? (
          <div className="flex items-center gap-2 rounded-control border border-border bg-surface-muted px-3 py-2 text-sm text-ink">
            <LoaderCircle size={15} className="animate-spin text-accent" />
            正在运行，结果会写入新的 Agent 会话。
          </div>
        ) : null}

        <section className="grid gap-1.5">
          <h2 className="m-0 text-xs font-semibold uppercase tracking-wide text-ink-muted">任务说明</h2>
          <p className="m-0 whitespace-pre-wrap text-sm leading-relaxed text-ink">{task.prompt}</p>
        </section>

        <section className="grid gap-1.5">
          <h2 className="m-0 text-xs font-semibold uppercase tracking-wide text-ink-muted">知识库范围</h2>
          <div className="flex flex-wrap gap-1.5">
            {scopeNames.length ? (
              scopeNames.map((name) => (
                <span key={name} className="rounded-full bg-surface-muted px-2 py-0.5 text-xs text-ink">
                  {name}
                </span>
              ))
            ) : (
              <span className="text-xs text-ink-soft">
                {task.knowledgeBaseIds.length ? "绑定的知识库已失效" : "未绑定知识库"}
              </span>
            )}
          </div>
        </section>

        <section className="grid gap-2">
          <div className="flex items-center justify-between gap-2">
            <h2 className="m-0 text-xs font-semibold uppercase tracking-wide text-ink-muted">运行历史</h2>
            <span className="text-[11px] text-ink-soft">最近 {runs.length} 次</span>
          </div>
          {runsLoading ? (
            <p className="m-0 text-xs text-ink-muted">正在读取运行记录…</p>
          ) : runsError ? (
            <p className="m-0 text-xs text-danger">{runsError}</p>
          ) : runs.length ? (
            <ul className="m-0 grid list-none gap-1 p-0">
              {runs.map((run) => (
                <li key={run.id}>
                  <div className="flex items-center gap-2 rounded-control border border-transparent px-2 py-1.5 hover:border-border">
                    <RunStatusIcon status={run.status} />
                    <div className="grid min-w-0 flex-1 gap-0.5">
                      <span className="text-xs font-medium text-ink">{runStatusLabel(run.status)}</span>
                      <span className="text-[11px] text-ink-muted">
                        {run.scheduledInstant.startsWith("manual:")
                          ? `手动补做 · ${formatRunTime(run.startedAt)}`
                          : `原定 ${formatRunTime(run.scheduledInstant)} · 实际 ${formatRunTime(run.startedAt)}`}
                      </span>
                      {run.error ? <span className="text-[11px] text-danger">{run.error}</span> : null}
                    </div>
                    {run.sessionId ? (
                      <Button size="compact" variant="text" onClick={() => onOpenSession(run.sessionId!)}>
                        查看会话
                      </Button>
                    ) : null}
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p className="m-0 text-xs text-ink-soft">还没有运行记录。</p>
          )}
        </section>
      </div>

      {confirmDelete ? (
        <ConfirmDialog
          title="删除任务"
          message="删除后不会再自动运行，运行记录会一并移除；已经生成的会话会保留。"
          confirmLabel="删除"
          tone="danger"
          isBusy={isBusy}
          onCancel={() => setConfirmDelete(false)}
          onConfirm={() => {
            setConfirmDelete(false);
            void onDelete(task.id);
          }}
        />
      ) : null}
    </div>
  );
}

function RunStatusIcon({ status }: { status: string }) {
  if (status === "running") {
    return <LoaderCircle size={13} className="shrink-0 animate-spin text-accent" />;
  }
  if (status === "ok" || status === "reminded") {
    return <CheckCircle2 size={13} className="shrink-0 text-success" />;
  }
  if (status === "skipped" || status === "failed" || status === "blocked") {
    return <AlertCircle size={13} className="shrink-0 text-danger" />;
  }
  return <span className="size-1.5 shrink-0 rounded-full bg-ink-soft" />;
}

function runStatusLabel(status: string): string {
  switch (status) {
    case "ok":
      return "已完成";
    case "reminded":
      return "已提醒";
    case "failed":
      return "失败";
    case "skipped":
      return "已错过";
    case "blocked":
      return "受阻";
    case "running":
      return "运行中";
    default:
      return status;
  }
}
