import { invokeLogged, isTauriRuntime } from "./runtime";
import {
  createMockScheduledJob,
  deleteMockScheduledJob,
  listMockScheduleBlueprints,
  listMockScheduledJobRuns,
  listMockScheduledJobs,
  triggerMockScheduledJob,
  updateMockScheduledJob,
} from "../mock/schedules";
import type {
  CreateScheduledJobPayload,
  ScheduleBlueprint,
  ScheduledJob,
  ScheduledJobRun,
  ScheduledJobUpdatedEvent,
  UpdateScheduledJobPayload,
} from "../types";

/** 读取全部定时任务。 */
export async function listScheduledJobs(): Promise<ScheduledJob[]> {
  if (!isTauriRuntime()) {
    return listMockScheduledJobs();
  }
  return invokeLogged<ScheduledJob[]>("list_scheduled_jobs");
}

/** 读取内置建议模板。 */
export async function listScheduleBlueprints(): Promise<ScheduleBlueprint[]> {
  if (!isTauriRuntime()) {
    return listMockScheduleBlueprints();
  }
  return invokeLogged<ScheduleBlueprint[]>("list_schedule_blueprints");
}

/** 创建定时任务。 */
export async function createScheduledJob(payload: CreateScheduledJobPayload): Promise<ScheduledJob> {
  if (!isTauriRuntime()) {
    return createMockScheduledJob(payload);
  }
  return invokeLogged<ScheduledJob>("create_scheduled_job", { payload });
}

/** 更新或暂停/恢复定时任务。 */
export async function updateScheduledJob(payload: UpdateScheduledJobPayload): Promise<ScheduledJob> {
  if (!isTauriRuntime()) {
    return updateMockScheduledJob(payload);
  }
  return invokeLogged<ScheduledJob>("update_scheduled_job", { payload });
}

/** 删除定时任务及其运行记录。 */
export async function deleteScheduledJob(jobId: string): Promise<void> {
  if (!isTauriRuntime()) {
    deleteMockScheduledJob(jobId);
    return;
  }
  await invokeLogged("delete_scheduled_job", { payload: { jobId } });
}

/** 读取某个任务的最近运行历史。 */
export async function listScheduledJobRuns(jobId: string, limit = 20): Promise<ScheduledJobRun[]> {
  if (!isTauriRuntime()) {
    return listMockScheduledJobRuns(jobId, limit);
  }
  return invokeLogged<ScheduledJobRun[]>("list_scheduled_job_runs", { payload: { jobId, limit } });
}

/** 立即运行一次，不消耗下次调度拍。 */
export async function triggerScheduledJob(jobId: string): Promise<ScheduledJob> {
  if (!isTauriRuntime()) {
    return triggerMockScheduledJob(jobId);
  }
  return invokeLogged<ScheduledJob>("trigger_scheduled_job", { payload: { jobId } });
}

/** 监听调度器推送的任务状态更新。 */
export async function listenScheduledJobUpdated(
  onEvent: (payload: ScheduledJobUpdatedEvent) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<ScheduledJobUpdatedEvent>("scheduled-job-updated", (event) => {
    onEvent(event.payload);
  });
  return unlisten;
}