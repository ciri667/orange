import { createLocalId } from "../id";
import type {
  CreateScheduledJobPayload,
  JobSchedule,
  ScheduleBlueprint,
  ScheduledJob,
  ScheduledJobRun,
  UpdateScheduledJobPayload,
} from "../types";

const mockJobs: ScheduledJob[] = [];
const mockRuns: ScheduledJobRun[] = [];

const MOCK_BLUEPRINTS: ScheduleBlueprint[] = [
  {
    key: "daily-briefing",
    name: "每日简报",
    description: "每个工作日检索知识库近期改动，整理成可扫读的简报。",
    prompt:
      "请检索当前知识库范围内最近 24 小时有更新的笔记，整理一份简洁的每日简报：今日重点、未完成事项、值得跟进的问题。用中文短段落，不要编造不存在的笔记内容。若没有任何更新，明确说明并给出 1-2 条基于现有笔记的建议。",
    schedule: { type: "weekdays", time: "08:00" },
    scheduleDisplay: "工作日 8:00",
    sessionPolicy: "newEachRun",
    outputMode: "session",
    needsTopic: false,
  },
  {
    key: "weekly-review",
    name: "每周回顾",
    description: "每周五把最近的笔记整理成简明的状态更新。",
    prompt:
      "请检索当前知识库范围内最近 7 天的笔记，写一份每周回顾：本周完成了什么、仍未完成的事项、建议下周优先关注的 3 件事。引用具体笔记标题，不要编造。",
    schedule: { type: "weekly", days: ["FR"], time: "16:00" },
    scheduleDisplay: "星期五 16:00",
    sessionPolicy: "newEachRun",
    outputMode: "session",
    needsTopic: false,
  },
  {
    key: "topic-watch",
    name: "主题跟进",
    description: "每个工作日查看指定主题在知识库中的新增，并标出需要关注的事项。",
    prompt:
      "请围绕主题「{topic}」检索当前知识库。只报告相对上次（若有会话历史）的新增或变化；没有新增时回复「暂无需要关注的更新」，不要重复旧内容。标出需要用户关注的事项。",
    schedule: { type: "weekdays", time: "09:00" },
    scheduleDisplay: "工作日 9:00",
    sessionPolicy: "continue",
    outputMode: "session",
    needsTopic: true,
  },
];

/** 浏览器开发态返回内置建议模板。 */
export function listMockScheduleBlueprints(): ScheduleBlueprint[] {
  return MOCK_BLUEPRINTS.map((item) => ({ ...item }));
}

/** 浏览器开发态列出内存中的定时任务。 */
export function listMockScheduledJobs(): ScheduledJob[] {
  return mockJobs.map((job) => ({ ...job }));
}

/** 浏览器开发态创建定时任务。 */
export function createMockScheduledJob(payload: CreateScheduledJobPayload): ScheduledJob {
  const now = new Date().toISOString();
  const job: ScheduledJob = {
    id: createLocalId("sched"),
    name: payload.name.trim(),
    prompt: payload.prompt.trim(),
    schedule: payload.schedule,
    scheduleDisplay: displaySchedule(payload.schedule),
    timezone: "local",
    knowledgeBaseIds: payload.knowledgeBaseIds,
    outputMode: payload.outputMode ?? "session",
    inboxKnowledgeBaseId: payload.inboxKnowledgeBaseId,
    inboxFolder: payload.inboxFolder ?? "定时产出",
    sessionPolicy: payload.sessionPolicy ?? "newEachRun",
    modelProviderId: payload.modelProviderId,
    modelId: payload.modelId,
    explicitSkillIds: payload.explicitSkillIds,
    enabled: payload.enabled ?? true,
    nextRunAt: estimateNextRun(payload.schedule),
    failureStreak: 0,
    createdAt: now,
    updatedAt: now,
  };
  mockJobs.unshift(job);
  return { ...job };
}

/** 浏览器开发态更新定时任务。 */
export function updateMockScheduledJob(payload: UpdateScheduledJobPayload): ScheduledJob {
  const index = mockJobs.findIndex((job) => job.id === payload.jobId);
  if (index < 0) {
    throw new Error("找不到该定时任务。");
  }
  const current = mockJobs[index];
  const schedule = payload.schedule ?? current.schedule;
  const next: ScheduledJob = {
    ...current,
    name: payload.name?.trim() || current.name,
    prompt: payload.prompt?.trim() || current.prompt,
    schedule,
    scheduleDisplay: displaySchedule(schedule),
    knowledgeBaseIds: payload.knowledgeBaseIds ?? current.knowledgeBaseIds,
    outputMode: payload.outputMode ?? current.outputMode,
    inboxKnowledgeBaseId: payload.inboxKnowledgeBaseId ?? current.inboxKnowledgeBaseId,
    inboxFolder: payload.inboxFolder ?? current.inboxFolder,
    sessionPolicy: payload.sessionPolicy ?? current.sessionPolicy,
    modelProviderId: payload.modelProviderId === undefined ? current.modelProviderId : payload.modelProviderId,
    modelId: payload.modelId === undefined ? current.modelId : payload.modelId,
    explicitSkillIds: payload.explicitSkillIds ?? current.explicitSkillIds,
    enabled: payload.enabled ?? current.enabled,
    nextRunAt: payload.schedule || payload.enabled === true ? estimateNextRun(schedule) : current.nextRunAt,
    updatedAt: new Date().toISOString(),
  };
  mockJobs[index] = next;
  return { ...next };
}

/** 浏览器开发态删除定时任务。 */
export function deleteMockScheduledJob(jobId: string) {
  const index = mockJobs.findIndex((job) => job.id === jobId);
  if (index < 0) {
    throw new Error("找不到该定时任务。");
  }
  mockJobs.splice(index, 1);
  for (let runIndex = mockRuns.length - 1; runIndex >= 0; runIndex -= 1) {
    if (mockRuns[runIndex].jobId === jobId) {
      mockRuns.splice(runIndex, 1);
    }
  }
}

/** 浏览器开发态立即运行：只登记一条成功记录，不真正调用模型。 */
export function triggerMockScheduledJob(jobId: string): ScheduledJob {
  const job = mockJobs.find((item) => item.id === jobId);
  if (!job) {
    throw new Error("找不到该定时任务。");
  }
  const now = new Date().toISOString();
  mockRuns.unshift({
    id: createLocalId("srun"),
    jobId,
    scheduledInstant: `manual:${now}`,
    startedAt: now,
    finishedAt: now,
    status: "ok",
  });
  job.lastRunAt = now;
  job.lastStatus = "ok";
  job.lastError = undefined;
  job.updatedAt = now;
  return { ...job };
}

/** 浏览器开发态运行历史。 */
export function listMockScheduledJobRuns(jobId: string, limit: number): ScheduledJobRun[] {
  return mockRuns.filter((run) => run.jobId === jobId).slice(0, limit);
}

function displaySchedule(schedule: JobSchedule): string {
  if (schedule.type === "daily") {
    return `每天 ${schedule.time.slice(0, 5)}`;
  }
  if (schedule.type === "weekdays") {
    return `工作日 ${schedule.time.slice(0, 5)}`;
  }
  if (schedule.type === "weekly") {
    return `每周 ${schedule.time.slice(0, 5)}`;
  }
  if (schedule.type === "hourly") {
    return schedule.intervalHours <= 1 ? "每小时" : `每 ${schedule.intervalHours} 小时`;
  }
  return `一次 ${schedule.at}`;
}

function estimateNextRun(schedule: JobSchedule): string {
  const now = new Date();
  const next = new Date(now.getTime() + 60 * 60 * 1000);
  if (schedule.type === "once") {
    const parsed = new Date(schedule.at);
    return Number.isNaN(parsed.getTime()) ? next.toISOString() : parsed.toISOString();
  }
  if (schedule.type === "hourly") {
    next.setMinutes(0, 0, 0);
    next.setHours(next.getHours() + Math.max(1, schedule.intervalHours));
    return next.toISOString();
  }
  const time = "time" in schedule ? schedule.time : "08:00";
  const [hours, minutes] = time.split(":").map(Number);
  const candidate = new Date(now);
  candidate.setHours(hours || 8, minutes || 0, 0, 0);
  if (candidate <= now) {
    candidate.setDate(candidate.getDate() + 1);
  }
  return candidate.toISOString();
}
