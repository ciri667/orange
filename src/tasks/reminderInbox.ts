import type { ScheduledJob } from "../shared/types";

/** 收件箱持久化键；升级数据结构时递增版本号，旧数据会被丢弃而不是解析出错。 */
export const REMINDER_INBOX_STORAGE_KEY = "orange.reminderInbox.v1";

/** 收件箱最多保留的提醒条数；超出后丢弃最旧的，避免无限增长。 */
export const REMINDER_INBOX_LIMIT = 20;

/** Toast 展示时长；到点自动收起，但收件箱里的未读仍然保留。 */
export const REMINDER_TOAST_TTL_MS = 20_000;

/** 预览文本最大字符数，Toast 和收件箱共用。 */
const PREVIEW_LIMIT = 80;

/**
 * 一条待处理的提醒。
 *
 * 只在内存和 localStorage 里维护「未读」语义；真正的持久记录仍是
 * scheduled_job_runs 里的 `reminded` 拍，这里只是把「用户还没看到」这件事记住。
 */
export interface ReminderNotice {
  /** 去重键：同一任务同一拍只保留一条。 */
  id: string;
  jobId: string;
  jobName: string;
  /** 提醒触发时间（ISO）。 */
  firedAt: string;
  /** 任务说明首行，用于 Toast 和收件箱预览。 */
  preview: string;
}

/** 从调度器推送的任务快照里提取提醒；非提醒任务返回 null。 */
export function reminderNoticeFromJob(job: ScheduledJob): ReminderNotice | null {
  if (job.outputMode !== "reminder" || job.lastStatus !== "reminded") {
    return null;
  }
  // lastRunAt 由后端在结束这一拍时写入，天然可以作为「同一拍」的稳定标识。
  const firedAt = job.lastRunAt ?? job.updatedAt;
  return {
    id: `${job.id}@${firedAt}`,
    jobId: job.id,
    jobName: job.name,
    firedAt,
    preview: buildPreview(job),
  };
}

/** 取任务说明首行作为预览；空说明退化为周期描述。 */
function buildPreview(job: ScheduledJob): string {
  const firstLine =
    job.prompt
      .split(/\r?\n/)
      .map((line) => line.trim())
      .find(Boolean) ?? "";
  const text = firstLine || job.scheduleDisplay;
  const chars = Array.from(text);
  return chars.length > PREVIEW_LIMIT ? `${chars.slice(0, PREVIEW_LIMIT).join("")}…` : text;
}

/** 新提醒插到最前并去重；同一拍重复推送不会产生两条。 */
export function mergeReminderNotice(
  current: ReminderNotice[],
  notice: ReminderNotice,
  limit = REMINDER_INBOX_LIMIT,
): ReminderNotice[] {
  const next = [notice, ...current.filter((item) => item.id !== notice.id)];
  return next.slice(0, limit);
}

/** 移除单条提醒。 */
export function dismissReminder(current: ReminderNotice[], id: string): ReminderNotice[] {
  return current.filter((item) => item.id !== id);
}

/** 移除某个任务的全部提醒；打开任务详情时用它清掉该任务的未读。 */
export function dismissRemindersForJob(
  current: ReminderNotice[],
  jobId: string,
): ReminderNotice[] {
  return current.filter((item) => item.jobId !== jobId);
}

/** 解析持久化内容；任何异常都退化成空收件箱，不阻塞启动。 */
export function parseReminderNotices(raw: string | null): ReminderNotice[] {
  if (!raw) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) {
      return [];
    }
    return parsed.filter(isReminderNotice).slice(0, REMINDER_INBOX_LIMIT);
  } catch {
    return [];
  }
}

/** 序列化收件箱；只写展示需要的字段。 */
export function serializeReminderNotices(notices: ReminderNotice[]): string {
  return JSON.stringify(notices.slice(0, REMINDER_INBOX_LIMIT));
}

/** 运行期校验，避免旧版本或手改过的 localStorage 把脏数据带进界面。 */
function isReminderNotice(value: unknown): value is ReminderNotice {
  if (!value || typeof value !== "object") {
    return false;
  }
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.id === "string" &&
    typeof candidate.jobId === "string" &&
    typeof candidate.jobName === "string" &&
    typeof candidate.firedAt === "string" &&
    typeof candidate.preview === "string"
  );
}
