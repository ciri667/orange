import type { JobSchedule, JobWeekday, ScheduleBlueprint, ScheduledJob } from "../shared/types";

const WEEKDAY_LABELS: Record<JobWeekday, string> = {
  MO: "星期一",
  TU: "星期二",
  WE: "星期三",
  TH: "星期四",
  FR: "星期五",
  SA: "星期六",
  SU: "星期日",
};

export const WEEKDAY_OPTIONS: Array<{ id: JobWeekday; label: string }> = [
  { id: "MO", label: "一" },
  { id: "TU", label: "二" },
  { id: "WE", label: "三" },
  { id: "TH", label: "四" },
  { id: "FR", label: "五" },
  { id: "SA", label: "六" },
  { id: "SU", label: "日" },
];

/** 把结构化周期格式化成列表短句。 */
export function formatScheduleDisplay(schedule: JobSchedule): string {
  if (schedule.type === "daily") {
    return `每天 ${clock(schedule.time)}`;
  }
  if (schedule.type === "weekdays") {
    return `工作日 ${clock(schedule.time)}`;
  }
  if (schedule.type === "weekly") {
    const names = schedule.days.map((day) => WEEKDAY_LABELS[day]).join("、");
    return names ? `${names} ${clock(schedule.time)}` : `每周 ${clock(schedule.time)}`;
  }
  if (schedule.type === "hourly") {
    const interval = schedule.intervalHours <= 1 ? "每小时" : `每 ${schedule.intervalHours} 小时`;
    if (schedule.days?.length) {
      return `${interval}（${schedule.days.map((day) => WEEKDAY_LABELS[day]).join("、")}）`;
    }
    return interval;
  }
  return `一次 ${schedule.at}`;
}

/** 把 UTC RFC3339 显示成本地时间。 */
export function formatRunAt(value?: string): string {
  if (!value) {
    return "未安排";
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(parsed);
}

/** 运行状态短标签。 */
export function formatJobStatus(status?: string): string {
  switch (status) {
    case "ok":
      return "成功";
    case "failed":
      return "失败";
    case "skipped":
      return "已跳过";
    case "blocked":
      return "已阻塞";
    case "running":
      return "运行中";
    default:
      return status ? status : "尚未运行";
  }
}

/** 把模板提示词里的 {topic} 换成用户填写的主题。 */
export function fillBlueprintPrompt(blueprint: ScheduleBlueprint, topic: string): string {
  if (!blueprint.needsTopic) {
    return blueprint.prompt;
  }
  const filled = topic.trim() || "用户关心的主题";
  return blueprint.prompt.replace(/\{topic\}/g, filled);
}

export function jobMatchesQuery(job: ScheduledJob, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return true;
  }
  return [job.name, job.prompt, job.scheduleDisplay].some((field) => field.toLowerCase().includes(needle));
}

function clock(time: string) {
  return time.trim().slice(0, 5);
}
