import { Bell, BookOpen, CalendarClock, Clock, Pause, Play, Plus, Search, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button } from "../shared/Button";
import { ConfirmDialog } from "../shared/ConfirmDialog";
import { ListRow } from "../shared/ListRow";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { cn } from "../shared/cn";
import { fieldControlClassName, focusShellClassName } from "../shared/ui";
import {
  createScheduledJob,
  deleteScheduledJob,
  listScheduleBlueprints,
  listScheduledJobRuns,
  listScheduledJobs,
  listenScheduledJobUpdated,
  triggerScheduledJob,
  updateScheduledJob,
} from "../shared/tauriApi";
import type { KnowledgeBase, ScheduleBlueprint, ScheduledJob, ScheduledJobRun } from "../shared/types";
import {
  ScheduleEditorDialog,
  draftFromJob,
  draftToSchedule,
  emptyDraft,
  type ScheduleEditorDraft,
} from "./ScheduleEditorDialog";
import { fillBlueprintPrompt, formatJobStatus, formatRunAt, jobMatchesQuery } from "./scheduleUtils";

const BLUEPRINT_ICONS = [Bell, BookOpen, CalendarClock];

/** 定时任务主视图：建议模板、任务列表、创建/暂停/立即运行。 */
export function SchedulesView({
  knowledgeBases,
  activeKnowledgeBaseId,
  isBusy,
  onBusy,
  onNotice,
  onOpenSession,
}: {
  knowledgeBases: KnowledgeBase[];
  activeKnowledgeBaseId: string;
  isBusy: boolean;
  onBusy: (label: string, task: () => Promise<void>) => Promise<void>;
  onNotice: (message: string) => void;
  onOpenSession: (sessionId: string) => void;
}) {
  const [jobs, setJobs] = useState<ScheduledJob[]>([]);
  const [blueprints, setBlueprints] = useState<ScheduleBlueprint[]>([]);
  const [query, setQuery] = useState("");
  const [selectedJobId, setSelectedJobId] = useState("");
  const [runs, setRuns] = useState<ScheduledJobRun[]>([]);
  const [createMenuOpen, setCreateMenuOpen] = useState(false);
  const [editor, setEditor] = useState<{ mode: "create" | "edit"; jobId?: string; draft: ScheduleEditorDraft } | null>(
    null,
  );
  const [editorError, setEditorError] = useState("");
  const [topicPrompt, setTopicPrompt] = useState<ScheduleBlueprint | null>(null);
  const [topic, setTopic] = useState("");
  const [pendingDeleteId, setPendingDeleteId] = useState("");

  const selectedJob = jobs.find((job) => job.id === selectedJobId) ?? jobs[0];

  const visibleJobs = useMemo(() => jobs.filter((job) => jobMatchesQuery(job, query)), [jobs, query]);

  useEffect(() => {
    void refreshJobs();
    void listScheduleBlueprints()
      .then(setBlueprints)
      .catch((error) => onNotice(error instanceof Error ? error.message : String(error)));
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listenScheduledJobUpdated((payload) => {
      setJobs((current) => {
        const index = current.findIndex((job) => job.id === payload.job.id);
        if (index < 0) {
          return [payload.job, ...current];
        }
        const next = current.slice();
        next[index] = payload.job;
        return next;
      });
      if (payload.job.id === selectedJobId || (!selectedJobId && payload.sessionId)) {
        void loadRuns(payload.job.id);
      }
    }).then((stop) => {
      unlisten = stop;
    });
    return () => unlisten?.();
  }, [selectedJobId]);

  useEffect(() => {
    if (!selectedJob) {
      setRuns([]);
      return;
    }
    void loadRuns(selectedJob.id);
  }, [selectedJob?.id]);

  async function refreshJobs() {
    try {
      const nextJobs = await listScheduledJobs();
      setJobs(nextJobs);
      if (selectedJobId && !nextJobs.some((job) => job.id === selectedJobId)) {
        setSelectedJobId(nextJobs[0]?.id ?? "");
      }
    } catch (error) {
      onNotice(error instanceof Error ? error.message : String(error));
    }
  }

  async function loadRuns(jobId: string) {
    try {
      setRuns(await listScheduledJobRuns(jobId));
    } catch (error) {
      onNotice(error instanceof Error ? error.message : String(error));
    }
  }

  function openCreate(draft: ScheduleEditorDraft) {
    setEditorError("");
    setEditor({ mode: "create", draft });
    setCreateMenuOpen(false);
  }

  function openBlueprint(blueprint: ScheduleBlueprint) {
    setCreateMenuOpen(false);
    if (blueprint.needsTopic) {
      setTopic("");
      setTopicPrompt(blueprint);
      return;
    }
    openCreate(draftFromBlueprint(blueprint, "", activeKnowledgeBaseId));
  }

  async function submitEditor() {
    if (!editor) {
      return;
    }
    const draft = editor.draft;
    if (!draft.name.trim() || !draft.prompt.trim()) {
      setEditorError("请填写名称和任务说明。");
      return;
    }
    if (!draft.knowledgeBaseIds.length) {
      setEditorError("请至少选择一个知识库。");
      return;
    }
    await onBusy(editor.mode === "create" ? "正在创建定时任务..." : "正在保存定时任务...", async () => {
      const schedule = draftToSchedule(draft);
      const saved =
        editor.mode === "create"
          ? await createScheduledJob({
              name: draft.name,
              prompt: draft.prompt,
              schedule,
              knowledgeBaseIds: draft.knowledgeBaseIds,
              sessionPolicy: draft.sessionPolicy,
              outputMode: "session",
            })
          : await updateScheduledJob({
              jobId: editor.jobId ?? "",
              name: draft.name,
              prompt: draft.prompt,
              schedule,
              knowledgeBaseIds: draft.knowledgeBaseIds,
              sessionPolicy: draft.sessionPolicy,
            });
      setJobs((current) => {
        const without = current.filter((job) => job.id !== saved.id);
        return [saved, ...without];
      });
      setSelectedJobId(saved.id);
      setEditor(null);
      onNotice(editor.mode === "create" ? "已创建定时任务。" : "已保存定时任务。");
    });
  }

  async function toggleEnabled(job: ScheduledJob) {
    await onBusy(job.enabled ? "正在暂停定时任务..." : "正在恢复定时任务...", async () => {
      const saved = await updateScheduledJob({ jobId: job.id, enabled: !job.enabled });
      setJobs((current) => current.map((item) => (item.id === saved.id ? saved : item)));
    });
  }

  async function runNow(job: ScheduledJob) {
    await onBusy("正在启动定时任务...", async () => {
      const saved = await triggerScheduledJob(job.id);
      setJobs((current) => current.map((item) => (item.id === saved.id ? saved : item)));
      await loadRuns(saved.id);
      onNotice(`已开始运行「${job.name}」。应用需保持在后台。`);
    });
  }

  async function confirmDelete() {
    const jobId = pendingDeleteId;
    if (!jobId) {
      return;
    }
    await onBusy("正在删除定时任务...", async () => {
      await deleteScheduledJob(jobId);
      setJobs((current) => current.filter((job) => job.id !== jobId));
      setPendingDeleteId("");
      onNotice("已删除定时任务。");
    });
  }

  return (
    <section className="grid min-h-0 min-w-0 flex-1 grid-rows-[auto_minmax(0,1fr)] bg-surface" aria-label="定时任务">
      <header className="flex items-start justify-between gap-3 border-b border-border px-6 py-5 max-[760px]:px-4">
        <div className="grid min-w-0 gap-1">
          <h1 className="m-0 text-[28px] font-semibold tracking-tight text-ink-strong">定时任务</h1>
          <p className="m-0 text-sm text-ink-muted">让橘记按点检索知识库、整理笔记并写进会话</p>
        </div>
        <Menu open={createMenuOpen} onClose={() => setCreateMenuOpen(false)}>
          <Button variant="primary" onClick={() => setCreateMenuOpen((open) => !open)}>
            <Plus size={16} />
            创建
          </Button>
          {createMenuOpen ? (
            <MenuPanel>
              <MenuItem onClick={() => openCreate(emptyDraft(activeKnowledgeBaseId))}>自定义任务</MenuItem>
              {blueprints.map((blueprint) => (
                <MenuItem key={blueprint.key} onClick={() => openBlueprint(blueprint)}>
                  {blueprint.name}
                </MenuItem>
              ))}
            </MenuPanel>
          ) : null}
        </Menu>
      </header>

      <div className="grid min-h-0 grid-cols-[minmax(0,1fr)_minmax(240px,320px)] max-[960px]:grid-cols-1">
        <div className="grid min-h-0 content-start gap-6 overflow-auto px-6 py-5 max-[760px]:px-4">
          <label className={cn("flex min-h-10 items-center gap-2 rounded-full border border-border bg-surface px-3 text-ink-muted", focusShellClassName)}>
            <Search size={16} />
            <input
              className="min-w-0 w-full border-0 bg-transparent text-sm text-ink outline-0"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索已安排任务"
              type="search"
            />
          </label>

          {!jobs.length ? (
            <section className="grid gap-3" aria-label="建议">
              <p className="m-0 text-sm font-medium text-ink-muted">建议</p>
              {blueprints.map((blueprint, index) => {
                const Icon = BLUEPRINT_ICONS[index] ?? Clock;
                return (
                  <button
                    key={blueprint.key}
                    type="button"
                    className="grid gap-1 rounded-xl border border-transparent px-1 py-2 text-left hover:bg-surface-hover"
                    onClick={() => openBlueprint(blueprint)}
                  >
                    <span className="flex items-center gap-2 text-[15px] font-medium text-ink-strong">
                      <Icon size={16} className="text-ink-muted" />
                      {blueprint.name}
                      <span className="font-normal text-ink-muted">{blueprint.scheduleDisplay}</span>
                    </span>
                    <span className="pl-6 text-sm leading-normal text-ink-muted">{blueprint.description}</span>
                  </button>
                );
              })}
            </section>
          ) : (
            <section className="grid gap-1" aria-label="已安排任务">
              {visibleJobs.map((job) => (
                <ListRow
                  key={job.id}
                  active={job.id === selectedJob?.id}
                  className="items-start py-2.5"
                  onClick={() => setSelectedJobId(job.id)}
                >
                  <Clock size={16} className={job.enabled ? "mt-0.5 text-accent" : "mt-0.5 text-ink-soft"} />
                  <span className="grid min-w-0 flex-1 gap-0.5">
                    <span className="flex min-w-0 items-center gap-2">
                      <OverflowTooltipText
                        as="span"
                        className="min-w-0 truncate text-[14px] font-medium text-ink-strong"
                        text={job.name}
                        logArea="scheduled_job_name"
                      />
                      <span className="shrink-0 text-xs text-ink-muted">{job.scheduleDisplay}</span>
                    </span>
                    <span className="text-xs text-ink-muted">
                      {job.enabled ? `下次 ${formatRunAt(job.nextRunAt)}` : "已暂停"}
                      {job.lastStatus ? ` · 上次${formatJobStatus(job.lastStatus)}` : ""}
                    </span>
                  </span>
                </ListRow>
              ))}
              {!visibleJobs.length ? <p className="m-0 px-2 text-sm text-ink-muted">没有匹配的任务。</p> : null}
            </section>
          )}
        </div>

        {selectedJob ? (
          <aside className="grid min-h-0 content-start gap-4 overflow-auto border-l border-border px-4 py-5 max-[960px]:border-l-0 max-[960px]:border-t">
            <div className="grid gap-1">
              <h2 className="m-0 text-base font-semibold text-ink-strong">{selectedJob.name}</h2>
              <p className="m-0 text-xs text-ink-muted">{selectedJob.scheduleDisplay}</p>
            </div>
            <p className="m-0 whitespace-pre-wrap text-sm leading-relaxed text-ink">{selectedJob.prompt}</p>
            {selectedJob.lastError ? <p className="m-0 text-xs text-danger">{selectedJob.lastError}</p> : null}
            <div className="flex flex-wrap gap-2">
              <Button size="compact" onClick={() => void runNow(selectedJob)} disabled={isBusy}>
                <Play size={14} />
                立即运行
              </Button>
              <Button size="compact" onClick={() => void toggleEnabled(selectedJob)} disabled={isBusy}>
                {selectedJob.enabled ? <Pause size={14} /> : <Play size={14} />}
                {selectedJob.enabled ? "暂停" : "恢复"}
              </Button>
              <Button
                size="compact"
                onClick={() => {
                  setEditorError("");
                  setEditor({
                    mode: "edit",
                    jobId: selectedJob.id,
                    draft: draftFromJob(selectedJob, activeKnowledgeBaseId),
                  });
                }}
              >
                编辑
              </Button>
              <Button size="compact" tone="danger" onClick={() => setPendingDeleteId(selectedJob.id)}>
                <Trash2 size={14} />
                删除
              </Button>
            </div>
            <section className="grid gap-2">
              <p className="m-0 text-xs font-medium text-ink-muted">运行历史</p>
              {runs.length ? (
                <ul className="m-0 grid list-none gap-1 p-0">
                  {runs.map((run) => (
                    <li key={run.id}>
                      <button
                        type="button"
                        className="flex w-full items-center justify-between gap-2 rounded-control px-1 py-1.5 text-left text-xs text-ink-muted hover:bg-surface-hover"
                        disabled={!run.sessionId}
                        onClick={() => run.sessionId && onOpenSession(run.sessionId)}
                      >
                        <span>
                          {formatJobStatus(run.status)} · {formatRunAt(run.startedAt)}
                        </span>
                        {run.sessionId ? <span className="text-ink">查看会话</span> : null}
                      </button>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="m-0 text-xs text-ink-soft">还没有运行记录。</p>
              )}
            </section>
          </aside>
        ) : null}
      </div>

      {editor ? (
        <ScheduleEditorDialog
          title={editor.mode === "create" ? "创建定时任务" : "编辑定时任务"}
          draft={editor.draft}
          knowledgeBases={knowledgeBases}
          isBusy={isBusy}
          error={editorError}
          onChange={(draft) => setEditor({ ...editor, draft })}
          onClose={() => setEditor(null)}
          onSubmit={submitEditor}
        />
      ) : null}

      {topicPrompt ? (
        <TopicPromptDialog
          blueprint={topicPrompt}
          topic={topic}
          onTopicChange={setTopic}
          onClose={() => setTopicPrompt(null)}
          onConfirm={() => {
            openCreate(draftFromBlueprint(topicPrompt, topic, activeKnowledgeBaseId));
            setTopicPrompt(null);
          }}
        />
      ) : null}

      {pendingDeleteId ? (
        <ConfirmDialog
          title="删除定时任务"
          message="删除后不会再自动运行。已经生成的会话会保留。"
          confirmLabel="删除"
          tone="danger"
          isBusy={isBusy}
          onCancel={() => setPendingDeleteId("")}
          onConfirm={() => void confirmDelete()}
        />
      ) : null}
    </section>
  );
}

function draftFromBlueprint(
  blueprint: ScheduleBlueprint,
  topic: string,
  knowledgeBaseId: string,
): ScheduleEditorDraft {
  const draft = emptyDraft(knowledgeBaseId);
  draft.name = blueprint.needsTopic && topic.trim() ? `${blueprint.name} · ${topic.trim()}` : blueprint.name;
  draft.prompt = fillBlueprintPrompt(blueprint, topic);
  draft.sessionPolicy = blueprint.sessionPolicy === "continue" ? "continue" : "newEachRun";
  if (blueprint.schedule.type === "weekdays") {
    draft.scheduleKind = "weekdays";
    draft.time = blueprint.schedule.time;
  } else if (blueprint.schedule.type === "weekly") {
    draft.scheduleKind = "weekly";
    draft.time = blueprint.schedule.time;
    draft.days = blueprint.schedule.days;
  } else if (blueprint.schedule.type === "daily") {
    draft.scheduleKind = "daily";
    draft.time = blueprint.schedule.time;
  } else if (blueprint.schedule.type === "hourly") {
    draft.scheduleKind = "hourly";
    draft.intervalHours = blueprint.schedule.intervalHours;
  }
  return draft;
}

function TopicPromptDialog({
  blueprint,
  topic,
  onTopicChange,
  onClose,
  onConfirm,
}: {
  blueprint: ScheduleBlueprint;
  topic: string;
  onTopicChange: (value: string) => void;
  onClose: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="fixed inset-0 z-modal grid place-items-center bg-[rgba(17,24,39,0.36)]" role="presentation" onMouseDown={onClose}>
      <form
        className="grid w-[min(420px,calc(100vw-40px))] gap-3 rounded-2xl border border-border bg-surface p-4 shadow-app"
        aria-label="填写跟进主题"
        onMouseDown={(event) => event.stopPropagation()}
        onSubmit={(event) => {
          event.preventDefault();
          onConfirm();
        }}
      >
        <h2 className="m-0 text-base font-semibold text-ink-strong">{blueprint.name}</h2>
        <p className="m-0 text-sm text-ink-muted">{blueprint.description}</p>
        <input
          className={fieldControlClassName}
          value={topic}
          onChange={(event) => onTopicChange(event.target.value)}
          placeholder="例如：LLM 微调、竞品动态"
          autoFocus
        />
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            取消
          </Button>
          <Button type="submit" variant="primary" disabled={!topic.trim()}>
            继续
          </Button>
        </div>
      </form>
    </div>
  );
}
