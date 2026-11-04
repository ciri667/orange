import { type FormEvent, useState } from "react";
import { Sparkles } from "lucide-react";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import { ModalBackdrop, ModalPanel } from "../shared/Modal";
import { SegmentedControl, SegmentedControlItem } from "../shared/SegmentedControl";
import { SelectControl } from "../shared/SelectControl";
import { fieldControlClassName, fieldLabelClassName, fieldTextareaClassName } from "../shared/ui";
import type {
  JobSchedule,
  JobWeekday,
  KnowledgeBase,
  ScheduleBlueprint,
  ScheduledJob,
  TaskKind,
} from "../shared/types";
import { fillBlueprintPrompt, WEEKDAY_OPTIONS } from "../schedules/scheduleUtils";
import {
  agendaAtForEditor,
  defaultAgendaAt,
  parseNaturalLanguageTime,
  splitRecurringSchedule,
} from "./taskUtils";

export interface TaskEditorDraft {
  kind: TaskKind;
  name: string;
  prompt: string;
  onceAt: string;
  scheduleKind: Exclude<JobSchedule["type"], "once">;
  time: string;
  intervalHours: number;
  days: JobWeekday[];
  knowledgeBaseIds: string[];
  sessionPolicy: "newEachRun" | "continue";
  /** 只发提醒，不运行 Agent；仍按日程/定时规则触发。 */
  reminderOnly: boolean;
}

export function emptyTaskDraft(knowledgeBaseId: string, kind: TaskKind = "agenda"): TaskEditorDraft {
  return {
    kind,
    name: "",
    prompt: "",
    onceAt: defaultAgendaAt(),
    scheduleKind: "weekdays",
    time: "09:00",
    intervalHours: 2,
    days: ["FR"],
    knowledgeBaseIds: knowledgeBaseId ? [knowledgeBaseId] : [],
    sessionPolicy: "newEachRun",
    reminderOnly: false,
  };
}

export function taskDraftFromJob(job: ScheduledJob, fallbackKnowledgeBaseId: string): TaskEditorDraft {
  const recurring = splitRecurringSchedule(job.schedule);
  return {
    kind: job.schedule.type === "once" ? "agenda" : "scheduled",
    name: job.name,
    prompt: job.prompt,
    onceAt: agendaAtForEditor(job),
    ...recurring,
    knowledgeBaseIds: job.knowledgeBaseIds.length
      ? job.knowledgeBaseIds
      : [fallbackKnowledgeBaseId].filter(Boolean),
    sessionPolicy: job.sessionPolicy === "continue" ? "continue" : "newEachRun",
    reminderOnly: job.outputMode === "reminder",
  };
}

export function draftToSchedule(draft: TaskEditorDraft): JobSchedule {
  if (draft.kind === "agenda") {
    return { type: "once", at: draft.onceAt };
  }
  if (draft.scheduleKind === "hourly") {
    return { type: "hourly", intervalHours: Math.min(24, Math.max(1, draft.intervalHours)) };
  }
  if (draft.scheduleKind === "daily") {
    return { type: "daily", time: draft.time || "09:00" };
  }
  if (draft.scheduleKind === "weekly") {
    return {
      type: "weekly",
      days: draft.days.length ? draft.days : ["FR"],
      time: draft.time || "09:00",
    };
  }
  return { type: "weekdays", time: draft.time || "09:00" };
}

function suggestTaskName(prompt: string): string {
  const firstLine = prompt
    .trim()
    .split(/\r?\n/)
    .find((line) => line.trim())
    ?.trim();
  if (!firstLine) {
    return "未命名任务";
  }
  return firstLine.length > 26 ? `${firstLine.slice(0, 26)}…` : firstLine;
}

/** 新建/编辑任务的统一弹窗：日程填日期时间，定时填重复规则。 */
export function TaskEditorDialog({
  title,
  draft,
  knowledgeBases,
  blueprints = [],
  isBusy,
  error,
  onChange,
  onClose,
  onSubmit,
}: {
  title: string;
  draft: TaskEditorDraft;
  knowledgeBases: KnowledgeBase[];
  blueprints?: ScheduleBlueprint[];
  isBusy: boolean;
  error: string;
  onChange: (next: TaskEditorDraft) => void;
  onClose: () => void;
  onSubmit: (draft: TaskEditorDraft) => void | Promise<void>;
}) {
  const [naturalInput, setNaturalInput] = useState("");
  const [naturalError, setNaturalError] = useState("");

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    void onSubmit({
      ...draft,
      name: draft.name.trim() || suggestTaskName(draft.prompt),
    });
  }

  function changeKind(kind: TaskKind) {
    onChange({
      ...draft,
      kind,
      onceAt: kind === "agenda" && !draft.onceAt ? defaultAgendaAt() : draft.onceAt,
    });
  }

  function applyBlueprint(blueprint: ScheduleBlueprint, topic = "") {
    const recurring = splitRecurringSchedule(blueprint.schedule);
    onChange({
      ...draft,
      kind: "scheduled",
      name: blueprint.needsTopic && topic.trim() ? `${blueprint.name} · ${topic.trim()}` : blueprint.name,
      prompt: fillBlueprintPrompt(blueprint, topic),
      ...recurring,
      sessionPolicy: blueprint.sessionPolicy === "continue" ? "continue" : "newEachRun",
      reminderOnly: false,
    });
  }

  function applyNaturalLanguage() {
    const parsed = parseNaturalLanguageTime(naturalInput);
    if (!parsed) {
      setNaturalError("没读懂时间，试试「明天下午三点」或「周五上午十点」。");
      return;
    }
    setNaturalError("");
    const cleaned = naturalInput
      .replace(/(今天|明天|后天|大后天|周[一二三四五六日天])/g, "")
      .replace(
        /(上午|下午|晚上|中午)?\s*(?:\d{1,2}[:：]\d{1,2}|(?:\d{1,2}|十[一二]?|[一二三四五六七八九])\s*点)/g,
        "",
      )
      .trim();
    onChange({
      ...draft,
      kind: "agenda",
      onceAt: parsed,
      name: draft.name.trim() || (cleaned ? suggestTaskName(cleaned) : draft.name),
      prompt: draft.prompt.trim() ? draft.prompt : cleaned,
    });
  }

  return (
    <ModalBackdrop onClose={onClose}>
      <ModalPanel
        aria-label={title}
        className="flex max-h-[min(760px,calc(100vh-32px))] w-[min(560px,calc(100vw-40px))] flex-col"
      >
        <form className="flex min-h-0 flex-1 flex-col" onSubmit={handleSubmit}>
          <header className="shrink-0 border-b border-border px-4 py-3">
            <h2 className="m-0 text-base font-semibold text-ink-strong">{title}</h2>
            <p className="m-0 text-xs text-ink-muted">日程只运行一次；定时任务按重复规则运行。</p>
          </header>

          <div className="grid min-h-0 flex-1 content-start gap-3.5 overflow-y-auto px-4 py-3">
        <SegmentedControl aria-label="任务类型" role="radiogroup" className="w-fit">
          {(
            [
              ["agenda", "日程"],
              ["scheduled", "定时任务"],
            ] as const
          ).map(([kind, label]) => (
            <SegmentedControlItem
              key={kind}
              active={draft.kind === kind}
              role="radio"
              aria-checked={draft.kind === kind}
              className="px-3"
              onClick={() => changeKind(kind)}
            >
              {label}
            </SegmentedControlItem>
          ))}
        </SegmentedControl>

        <label className="flex items-center gap-2 text-sm text-ink">
          <Checkbox
            checked={draft.reminderOnly}
            onChange={(event) => onChange({ ...draft, reminderOnly: event.target.checked })}
          />
          <span>仅提醒，不运行 Agent</span>
          <span className="text-xs text-ink-muted">到点只发应用内提醒</span>
        </label>

        <label className={fieldLabelClassName}>
          <span>任务名称</span>
          <input
            className={fieldControlClassName}
            value={draft.name}
            onChange={(event) => onChange({ ...draft, name: event.target.value })}
            placeholder="留空会自动取任务说明首行"
            autoFocus
          />
        </label>

        <label className={fieldLabelClassName}>
          <span>要让橘记做什么？</span>
          <textarea
            className={fieldTextareaClassName}
            value={draft.prompt}
            onChange={(event) => onChange({ ...draft, prompt: event.target.value })}
            placeholder="例如：整理今天的会议记录，列出行动项和负责人"
            rows={4}
          />
        </label>

        <div className="grid gap-2 rounded-control border border-border bg-surface-muted/60 p-2.5">
          <div className="flex items-center gap-1.5 text-xs font-medium text-ink-muted">
            <Sparkles size={13} />
            一句话创建日程
          </div>
          <div className="flex gap-2">
            <input
              className={fieldControlClassName}
              value={naturalInput}
              onChange={(event) => setNaturalInput(event.target.value)}
              placeholder="明天下午三点整理会议纪要"
            />
            <Button type="button" size="compact" onClick={applyNaturalLanguage} disabled={!naturalInput.trim()}>
              解析
            </Button>
          </div>
          {naturalError ? <p className="m-0 text-xs text-danger">{naturalError}</p> : null}
        </div>

        {draft.kind === "agenda" ? (
          <label className={fieldLabelClassName}>
            <span>日期与时间</span>
            <input
              className={fieldControlClassName}
              type="datetime-local"
              value={draft.onceAt}
              onChange={(event) => onChange({ ...draft, onceAt: event.target.value })}
            />
          </label>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-3 max-[560px]:grid-cols-1">
              <label className={fieldLabelClassName}>
                <span>重复规则</span>
                <SelectControl
                  value={draft.scheduleKind}
                  onChange={(event) =>
                    onChange({
                      ...draft,
                      scheduleKind: event.target.value as TaskEditorDraft["scheduleKind"],
                    })
                  }
                >
                  <option value="weekdays">工作日</option>
                  <option value="daily">每天</option>
                  <option value="weekly">每周</option>
                  <option value="hourly">按小时</option>
                </SelectControl>
              </label>
              {draft.scheduleKind === "hourly" ? (
                <label className={fieldLabelClassName}>
                  <span>间隔（小时）</span>
                  <input
                    className={fieldControlClassName}
                    type="number"
                    min={1}
                    max={24}
                    value={draft.intervalHours}
                    onChange={(event) =>
                      onChange({ ...draft, intervalHours: Number(event.target.value) || 1 })
                    }
                  />
                </label>
              ) : (
                <label className={fieldLabelClassName}>
                  <span>时刻</span>
                  <input
                    className={fieldControlClassName}
                    type="time"
                    value={draft.time}
                    onChange={(event) => onChange({ ...draft, time: event.target.value })}
                  />
                </label>
              )}
            </div>
            {draft.scheduleKind === "weekly" ? (
              <fieldset className="m-0 grid min-w-0 gap-2 border-0 p-0">
                <legend className="p-0 text-xs font-bold text-ink-muted">星期</legend>
                <div className="flex flex-wrap gap-2">
                  {WEEKDAY_OPTIONS.map((option) => (
                    <label key={option.id} className="flex items-center gap-1.5 text-sm text-ink">
                      <Checkbox
                        checked={draft.days.includes(option.id)}
                        onChange={(event) => {
                          const nextDays = event.target.checked
                            ? [...draft.days, option.id]
                            : draft.days.filter((day) => day !== option.id);
                          onChange({ ...draft, days: nextDays });
                        }}
                      />
                      {option.label}
                    </label>
                  ))}
                </div>
              </fieldset>
            ) : null}
          </>
        )}

        {blueprints.length ? (
          <div className="grid gap-1.5">
            <p className="m-0 text-xs font-bold text-ink-muted">从模板开始</p>
            <div className="flex flex-wrap gap-1.5">
              {blueprints.map((blueprint) => (
                <Button
                  key={blueprint.key}
                  type="button"
                  size="compact"
                  onClick={() => applyBlueprint(blueprint)}
                >
                  {blueprint.name}
                </Button>
              ))}
            </div>
          </div>
        ) : null}

        <fieldset className="m-0 grid min-w-0 gap-2 border-0 p-0">
          <legend className="p-0 text-xs font-bold text-ink-muted">知识库范围</legend>
          <div className="grid max-h-36 gap-1.5 overflow-auto max-[520px]:grid-cols-1 min-[521px]:grid-cols-2">
            {knowledgeBases.map((knowledgeBase) => {
              const checked = draft.knowledgeBaseIds.includes(knowledgeBase.id);
              return (
                <label key={knowledgeBase.id} className="flex items-center gap-2 text-sm text-ink">
                  <Checkbox
                    checked={checked}
                    onChange={(event) => {
                      const nextIds = event.target.checked
                        ? [...draft.knowledgeBaseIds, knowledgeBase.id]
                        : draft.knowledgeBaseIds.filter((id) => id !== knowledgeBase.id);
                      onChange({ ...draft, knowledgeBaseIds: nextIds });
                    }}
                  />
                  {knowledgeBase.name}
                </label>
              );
            })}
          </div>
        </fieldset>

        {draft.reminderOnly ? null : (
          <label className={fieldLabelClassName}>
            <span>会话</span>
            <SelectControl
              value={draft.sessionPolicy}
              onChange={(event) =>
                onChange({
                  ...draft,
                  sessionPolicy: event.target.value as TaskEditorDraft["sessionPolicy"],
                })
              }
            >
              <option value="newEachRun">每次新建会话</option>
              <option value="continue">在同一会话续跑</option>
            </SelectControl>
          </label>
        )}



        {error ? <p className="m-0 text-xs text-danger">{error}</p> : null}
          </div>
          <footer className="flex shrink-0 justify-end gap-2 border-t border-border bg-surface px-4 py-3">
            <Button type="button" onClick={onClose} disabled={isBusy}>
              取消
            </Button>
            <Button type="submit" variant="primary" disabled={isBusy}>
              {isBusy ? "保存中…" : "保存任务"}
            </Button>
          </footer>
        </form>
      </ModalPanel>
    </ModalBackdrop>
  );
}
