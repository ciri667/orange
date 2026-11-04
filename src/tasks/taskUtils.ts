import type { JobSchedule, ScheduledJob, TaskKind, TaskStatus } from "../shared/types";

export type TaskFilter = "all" | TaskKind;

export interface TaskGroup {
  key: string;
  label: string;
  items: ScheduledJob[];
  tone?: "danger" | "muted";
}

const WEEKDAY_SHORT = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** 由调度规则派生任务类型：once 是一次性日程，其余是重复定时任务。 */
export function taskKind(task: ScheduledJob): TaskKind {
  return task.schedule.type === "once" ? "agenda" : "scheduled";
}

export function taskKindLabel(task: ScheduledJob): string {
  if (task.outputMode === "reminder") {
    return "提醒";
  }
  return taskKind(task) === "agenda" ? "日程" : "定时";
}

/** 一次性任务的原定时间；周期任务的下一次运行时间。 */
export function taskWhen(task: ScheduledJob): Date | null {
  const value =
    taskKind(task) === "agenda"
      ? task.scheduledAt ?? task.nextRunAt ?? task.lastRunAt
      : task.nextRunAt ?? task.lastRunAt;
  return parseDate(value);
}

export function taskStatus(task: ScheduledJob, now = new Date()): TaskStatus {
  if (task.lastStatus === "running") {
    return "running";
  }

  if (taskKind(task) === "agenda") {
    const next = parseDate(task.nextRunAt);
    if (next && next.getTime() > now.getTime()) {
      return "todo";
    }
    if (task.lastStatus === "ok") {
      return "done";
    }
    if (task.lastStatus === "reminded") {
      return "reminded";
    }
    if (task.lastStatus === "skipped") {
      return "missed";
    }
    if (task.lastStatus === "failed" || task.lastStatus === "blocked") {
      return "failed";
    }
    const planned = parseDate(task.scheduledAt);
    if (planned && planned.getTime() <= now.getTime()) {
      return "missed";
    }
    return "todo";
  }

  if (!task.enabled) {
    return "paused";
  }
  return "scheduled";
}

export function taskStatusLabel(status: TaskStatus): string {
  switch (status) {
    case "todo":
      return "待开始";
    case "scheduled":
      return "已安排";
    case "running":
      return "运行中";
    case "done":
      return "已完成";
    case "reminded":
      return "已提醒";
    case "missed":
      return "已错过";
    case "paused":
      return "已暂停";
    case "failed":
      return "失败";
  }
}

/** 周期任务仍有下一拍时，用副标签提示上一次失败/错过。 */
export function taskLastResultLabel(task: ScheduledJob): string {
  switch (task.lastStatus) {
    case "skipped":
      return "上次已错过";
    case "failed":
      return "上次失败";
    case "blocked":
      return "上次受阻";
    case "ok":
      return "上次成功";
    case "reminded":
      return "上次已提醒";
    default:
      return "";
  }
}

export function taskMatchesQuery(task: ScheduledJob, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return true;
  }
  return [task.name, task.prompt, task.scheduleDisplay].some((field) =>
    field.toLowerCase().includes(needle),
  );
}

/** 按日程/状态分组，组内按时间升序；已完成和已错过单独成组。 */
export function groupTasks(
  tasks: ScheduledJob[],
  filter: TaskFilter,
  query: string,
  now = new Date(),
): TaskGroup[] {
  const groups = new Map<string, TaskGroup>();
  const push = (key: string, label: string, task: ScheduledJob, tone?: TaskGroup["tone"]) => {
    const group = groups.get(key) ?? { key, label, items: [], tone };
    group.items.push(task);
    groups.set(key, group);
  };

  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startOfTomorrow = addDays(startOfToday, 1);
  const mondayIndex = (now.getDay() + 6) % 7;
  const endOfWeek = addDays(startOfToday, 7 - mondayIndex);

  for (const task of tasks) {
    if (filter !== "all" && taskKind(task) !== filter) {
      continue;
    }
    if (!taskMatchesQuery(task, query)) {
      continue;
    }

    const status = taskStatus(task, now);
    if (status === "missed") {
      push("missed", "已错过", task, "danger");
      continue;
    }
    if (status === "failed") {
      push("failed", "失败", task, "danger");
      continue;
    }
    if (status === "paused") {
      push("paused", "已暂停", task, "muted");
      continue;
    }
    if (status === "reminded") {
      push("reminded", "已提醒", task, "muted");
      continue;
    }
    if (status === "done") {
      push("done", "已完成", task, "muted");
      continue;
    }

    const when = taskWhen(task);
    if (!when) {
      push("later", "更晚", task);
      continue;
    }
    if (when < startOfTomorrow) {
      push("today", "今天", task);
    } else if (when < addDays(startOfTomorrow, 1)) {
      push("tomorrow", "明天", task);
    } else if (when < endOfWeek) {
      push("week", "本周", task);
    } else {
      push("later", "更晚", task);
    }
  }

  const order = ["today", "tomorrow", "week", "later", "missed", "failed", "paused", "reminded", "done"];
  return order
    .map((key) => groups.get(key))
    .filter((group): group is TaskGroup => Boolean(group))
    .map((group) => ({
      ...group,
      items: group.items.slice().sort((left, right) => {
        const leftTime = taskWhen(left)?.getTime() ?? 0;
        const rightTime = taskWhen(right)?.getTime() ?? 0;
        return group.key === "done" || group.key === "reminded" || group.key === "missed"
          ? rightTime - leftTime
          : leftTime - rightTime;
      }),
    }));
}

/** 需要用户注意的任务数：今天及已错过；用于侧栏角标。 */
export function taskAttentionCount(tasks: ScheduledJob[], now = new Date()): number {
  const endOfToday = addDays(new Date(now.getFullYear(), now.getMonth(), now.getDate()), 1);
  return tasks.filter((task) => {
    const status = taskStatus(task, now);
    if (status === "missed" || status === "failed" || task.lastStatus === "failed" || task.lastStatus === "blocked") {
      return true;
    }
    if (status === "done" || status === "reminded" || status === "paused") {
      return false;
    }
    const when = taskWhen(task);
    return Boolean(when && when < endOfToday && when >= new Date(now.getFullYear(), now.getMonth(), now.getDate()));
  }).length;
}

export function formatTaskWhen(task: ScheduledJob, now = new Date()): string {
  const when = taskWhen(task);
  if (!when) {
    return taskKind(task) === "agenda" ? "未设置时间" : "未安排下一次";
  }
  return formatDateLabel(when, now);
}

/** 列表行左侧时间：当天只显示 HH:mm，跨天补上日期。 */
export function formatTaskTime(task: ScheduledJob, now = new Date()): string {
  const when = taskWhen(task);
  if (!when) {
    return "--:--";
  }
  const sameDay =
    when.getFullYear() === now.getFullYear() &&
    when.getMonth() === now.getMonth() &&
    when.getDate() === now.getDate();
  return sameDay ? clock(when) : `${when.getMonth() + 1}/${when.getDate()} ${clock(when)}`;
}

export function formatRunTime(value?: string): string {
  const date = parseDate(value);
  return date ? formatDateLabel(date) : "未知时间";
}

export function formatDateTimeLocal(value: string | Date): string {
  const date = typeof value === "string" ? new Date(value) : value;
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const pad = (input: number) => String(input).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** 新建日程的默认时间：下一个整点；接近午夜则落到明天 9:00。 */
export function defaultAgendaAt(now = new Date()): string {
  const next = new Date(now);
  next.setMinutes(0, 0, 0);
  next.setHours(next.getHours() + 1);
  if (next.getHours() < 7) {
    next.setHours(9, 0, 0, 0);
  }
  return formatDateTimeLocal(next);
}

/** 简单的一句话时间解析：支持“今天/明天/后天/周X + 上午/下午/晚上 + 点”。 */
export function parseNaturalLanguageTime(text: string, now = new Date()): string | null {
  const trimmed = text.trim();
  if (!trimmed) {
    return null;
  }
  const dayMatch = trimmed.match(/(今天|明天|后天|大后天|周[一二三四五六日天])/);
  const timeMatch = trimmed.match(
    /(上午|下午|晚上|中午)?\s*(?:(\d{1,2})[:：](\d{1,2})|(\d{1,2}|十[一二]?|[一二三四五六七八九])\s*点)/,
  );
  if (!dayMatch || !timeMatch) {
    return null;
  }

  const date = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const dayToken = dayMatch[1];
  if (dayToken === "明天") {
    date.setDate(date.getDate() + 1);
  } else if (dayToken === "后天") {
    date.setDate(date.getDate() + 2);
  } else if (dayToken === "大后天") {
    date.setDate(date.getDate() + 3);
  } else if (dayToken.startsWith("周")) {
    const target = "日一二三四五六".indexOf(dayToken.slice(1));
    if (target >= 0) {
      const current = date.getDay();
      let delta = (target - current + 7) % 7;
      if (delta === 0) {
        delta = 7;
      }
      date.setDate(date.getDate() + delta);
    }
  }

  let hours = timeMatch[2] ? Number(timeMatch[2]) : parseHourToken(timeMatch[4] ?? "");
  const minutes = timeMatch[2] ? Number(timeMatch[3] ?? 0) : 0;
  const period = timeMatch[1];
  if ((period === "下午" || period === "晚上") && hours < 12) {
    hours += 12;
  }
  if (period === "中午" && hours < 11) {
    hours += 12;
  }
  if (hours > 23 || minutes > 59) {
    return null;
  }
  date.setHours(hours, minutes, 0, 0);
  return formatDateTimeLocal(date);
}

/** 把时间转成 “今天 15:00 / 明天 09:00 / 11月5日 周三 09:00”。 */
export function formatDateLabel(date: Date, now = new Date()): string {
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startOfTomorrow = addDays(startOfToday, 1);
  if (date >= startOfToday && date < startOfTomorrow) {
    return `今天 ${clock(date)}`;
  }
  if (date >= startOfTomorrow && date < addDays(startOfTomorrow, 1)) {
    return `明天 ${clock(date)}`;
  }
  return `${date.getMonth() + 1}月${date.getDate()}日 ${WEEKDAY_SHORT[date.getDay()]} ${clock(date)}`;
}

/** 一次性日程的编辑回填时间。 */
export function agendaAtForEditor(task: ScheduledJob): string {
  if (task.schedule.type === "once") {
    const parsed = new Date(task.schedule.at);
    if (!Number.isNaN(parsed.getTime())) {
      return formatDateTimeLocal(parsed);
    }
  }
  const when = taskWhen(task);
  return when ? formatDateTimeLocal(when) : defaultAgendaAt();
}

export function splitRecurringSchedule(schedule: JobSchedule): {
  scheduleKind: Exclude<JobSchedule["type"], "once">;
  time: string;
  intervalHours: number;
  days: Extract<JobSchedule, { type: "weekly" }>["days"];
} {
  if (schedule.type === "hourly") {
    return { scheduleKind: "hourly", time: "09:00", intervalHours: schedule.intervalHours, days: schedule.days ?? [] };
  }
  if (schedule.type === "weekly") {
    return { scheduleKind: "weekly", time: schedule.time, intervalHours: 2, days: schedule.days };
  }
  if (schedule.type === "daily") {
    return { scheduleKind: "daily", time: schedule.time, intervalHours: 2, days: [] };
  }
  if (schedule.type === "weekdays") {
    return { scheduleKind: "weekdays", time: schedule.time, intervalHours: 2, days: [] };
  }
  return { scheduleKind: "weekdays", time: "09:00", intervalHours: 2, days: [] };
}

function parseHourToken(token: string): number {
  const digits = Number(token);
  if (!Number.isNaN(digits)) {
    return digits;
  }
  const numerals: Record<string, number> = {
    一: 1,
    二: 2,
    三: 3,
    四: 4,
    五: 5,
    六: 6,
    七: 7,
    八: 8,
    九: 9,
    十: 10,
    十一: 11,
    十二: 12,
  };
  return numerals[token] ?? 0;
}

function parseDate(value?: string): Date | null {
  if (!value) {
    return null;
  }
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

function addDays(date: Date, days: number): Date {
  const next = new Date(date);
  next.setDate(next.getDate() + days);
  return next;
}

function clock(date: Date): string {
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}
