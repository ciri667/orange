import { type FormEvent } from "react";
import { Button } from "../shared/Button";
import { Checkbox } from "../shared/Checkbox";
import { ModalBackdrop, ModalForm } from "../shared/Modal";
import { SelectControl } from "../shared/SelectControl";
import { fieldControlClassName, fieldLabelClassName, fieldTextareaClassName } from "../shared/ui";
import type { JobSchedule, JobWeekday, KnowledgeBase, ScheduledJob } from "../shared/types";
import { WEEKDAY_OPTIONS } from "./scheduleUtils";

type ScheduleKind = JobSchedule["type"];

/** 创建或编辑定时任务的表单草稿。 */
export interface ScheduleEditorDraft {
  name: string;
  prompt: string;
  scheduleKind: ScheduleKind;
  time: string;
  intervalHours: number;
  days: JobWeekday[];
  onceAt: string;
  knowledgeBaseIds: string[];
  sessionPolicy: "newEachRun" | "continue";
}

export function draftFromJob(job: ScheduledJob, fallbackKnowledgeBaseId: string): ScheduleEditorDraft {
  return {
    name: job.name,
    prompt: job.prompt,
    ...splitSchedule(job.schedule),
    knowledgeBaseIds: job.knowledgeBaseIds.length ? job.knowledgeBaseIds : [fallbackKnowledgeBaseId],
    sessionPolicy: job.sessionPolicy === "continue" ? "continue" : "newEachRun",
  };
}

export function emptyDraft(knowledgeBaseId: string): ScheduleEditorDraft {
  return {
    name: "",
    prompt: "",
    scheduleKind: "weekdays",
    time: "09:00",
    intervalHours: 2,
    days: ["FR"],
    onceAt: "",
    knowledgeBaseIds: [knowledgeBaseId],
    sessionPolicy: "newEachRun",
  };
}

export function draftToSchedule(draft: ScheduleEditorDraft): JobSchedule {
  if (draft.scheduleKind === "hourly") {
    return { type: "hourly", intervalHours: Math.min(24, Math.max(1, draft.intervalHours)) };
  }
  if (draft.scheduleKind === "daily") {
    return { type: "daily", time: draft.time || "09:00" };
  }
  if (draft.scheduleKind === "weekly") {
    return { type: "weekly", days: draft.days.length ? draft.days : ["FR"], time: draft.time || "16:00" };
  }
  if (draft.scheduleKind === "once") {
    return { type: "once", at: draft.onceAt };
  }
  return { type: "weekdays", time: draft.time || "08:00" };
}

/** 新建/编辑定时任务弹窗。 */
export function ScheduleEditorDialog({
  title,
  draft,
  knowledgeBases,
  isBusy,
  error,
  onChange,
  onClose,
  onSubmit,
}: {
  title: string;
  draft: ScheduleEditorDraft;
  knowledgeBases: KnowledgeBase[];
  isBusy: boolean;
  error: string;
  onChange: (next: ScheduleEditorDraft) => void;
  onClose: () => void;
  onSubmit: () => void | Promise<void>;
}) {
  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    void onSubmit();
  }

  return (
    <ModalBackdrop onClose={onClose}>
      <ModalForm aria-label={title} className="w-[min(520px,calc(100vw-40px))]" onSubmit={handleSubmit}>
        <h2 className="m-0 text-base font-semibold text-ink-strong">{title}</h2>
        <label className={fieldLabelClassName}>
          <span>名称</span>
          <input
            className={fieldControlClassName}
            value={draft.name}
            onChange={(event) => onChange({ ...draft, name: event.target.value })}
            placeholder="例如：每日简报"
            autoFocus
          />
        </label>
        <label className={fieldLabelClassName}>
          <span>任务说明</span>
          <textarea
            className={fieldTextareaClassName}
            value={draft.prompt}
            onChange={(event) => onChange({ ...draft, prompt: event.target.value })}
            placeholder="告诉橘记到点后要做什么"
            rows={5}
          />
        </label>
        <div className="grid grid-cols-2 gap-3 max-[560px]:grid-cols-1">
          <label className={fieldLabelClassName}>
            <span>周期</span>
            <SelectControl
              value={draft.scheduleKind}
              onChange={(event) => onChange({ ...draft, scheduleKind: event.target.value as ScheduleKind })}
            >
              <option value="weekdays">工作日</option>
              <option value="daily">每天</option>
              <option value="weekly">每周</option>
              <option value="hourly">按小时</option>
              <option value="once">一次性</option>
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
                onChange={(event) => onChange({ ...draft, intervalHours: Number(event.target.value) || 1 })}
              />
            </label>
          ) : draft.scheduleKind === "once" ? (
            <label className={fieldLabelClassName}>
              <span>时间</span>
              <input
                className={fieldControlClassName}
                type="datetime-local"
                value={draft.onceAt}
                onChange={(event) => onChange({ ...draft, onceAt: event.target.value })}
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
          <fieldset className="grid gap-2">
            <legend className="text-xs font-bold text-ink-muted">星期</legend>
            <div className="flex flex-wrap gap-2">
              {WEEKDAY_OPTIONS.map((option) => {
                const checked = draft.days.includes(option.id);
                return (
                  <label key={option.id} className="flex items-center gap-1.5 text-sm text-ink">
                    <Checkbox
                      checked={checked}
                      onChange={(event) => {
                        const nextDays = event.target.checked
                          ? [...draft.days, option.id]
                          : draft.days.filter((day) => day !== option.id);
                        onChange({ ...draft, days: nextDays });
                      }}
                    />
                    {option.label}
                  </label>
                );
              })}
            </div>
          </fieldset>
        ) : null}
        <fieldset className="grid gap-2">
          <legend className="text-xs font-bold text-ink-muted">知识库范围</legend>
          <div className="grid gap-1.5">
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
        <label className={fieldLabelClassName}>
          <span>会话</span>
          <SelectControl
            value={draft.sessionPolicy}
            onChange={(event) =>
              onChange({ ...draft, sessionPolicy: event.target.value as ScheduleEditorDraft["sessionPolicy"] })
            }
          >
            <option value="newEachRun">每次新建会话</option>
            <option value="continue">在同一会话续跑</option>
          </SelectControl>
        </label>
        {error ? <p className="m-0 text-xs text-danger">{error}</p> : null}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose} disabled={isBusy}>
            取消
          </Button>
          <Button type="submit" variant="primary" disabled={isBusy}>
            保存
          </Button>
        </div>
      </ModalForm>
    </ModalBackdrop>
  );
}

function splitSchedule(schedule: JobSchedule): Pick<ScheduleEditorDraft, "scheduleKind" | "time" | "intervalHours" | "days" | "onceAt"> {
  if (schedule.type === "hourly") {
    return {
      scheduleKind: "hourly",
      time: "09:00",
      intervalHours: schedule.intervalHours,
      days: schedule.days ?? [],
      onceAt: "",
    };
  }
  if (schedule.type === "weekly") {
    return {
      scheduleKind: "weekly",
      time: schedule.time,
      intervalHours: 2,
      days: schedule.days,
      onceAt: "",
    };
  }
  if (schedule.type === "once") {
    return {
      scheduleKind: "once",
      time: "09:00",
      intervalHours: 2,
      days: ["FR"],
      onceAt: schedule.at,
    };
  }
  if (schedule.type === "daily") {
    return { scheduleKind: "daily", time: schedule.time, intervalHours: 2, days: ["FR"], onceAt: "" };
  }
  return { scheduleKind: "weekdays", time: schedule.time, intervalHours: 2, days: ["FR"], onceAt: "" };
}


