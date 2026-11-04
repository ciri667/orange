import { useCallback, useEffect, useRef, useState } from "react";
import {
  createScheduledJob,
  deleteScheduledJob,
  listScheduleBlueprints,
  listScheduledJobs,
  listenScheduledJobUpdated,
  triggerScheduledJob,
  updateScheduledJob,
} from "../shared/tauriApi";
import type {
  CreateScheduledJobPayload,
  ScheduleBlueprint,
  ScheduledJob,
  UpdateScheduledJobPayload,
} from "../shared/types";

/** 工作台级的任务数据源：侧栏列表、右侧详情和角标共用同一份状态。 */
export function useTasks() {
  const [tasks, setTasks] = useState<ScheduledJob[]>([]);
  const [blueprints, setBlueprints] = useState<ScheduleBlueprint[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState("");
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    setIsLoading(true);
    setError("");
    try {
      const next = await listScheduledJobs();
      if (mounted.current) {
        setTasks(next);
      }
    } catch (cause) {
      if (mounted.current) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    } finally {
      if (mounted.current) {
        setIsLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    void refresh();
    void listScheduleBlueprints()
      .then((next) => {
        if (mounted.current) {
          setBlueprints(next);
        }
      })
      .catch(() => {
        // 模板加载失败不阻塞任务列表；新建弹窗会退化为空白表单。
      });
  }, [refresh]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listenScheduledJobUpdated((payload) => {
      setTasks((current) => {
        const index = current.findIndex((task) => task.id === payload.job.id);
        if (index < 0) {
          return [payload.job, ...current];
        }
        const next = current.slice();
        next[index] = payload.job;
        return next;
      });
    }).then((stop) => {
      unlisten = stop;
    });
    return () => unlisten?.();
  }, []);

  const upsert = useCallback((task: ScheduledJob) => {
    setTasks((current) => {
      const index = current.findIndex((item) => item.id === task.id);
      if (index < 0) {
        return [task, ...current];
      }
      const next = current.slice();
      next[index] = task;
      return next;
    });
  }, []);

  const create = useCallback(
    async (payload: CreateScheduledJobPayload) => {
      const saved = await createScheduledJob(payload);
      upsert(saved);
      return saved;
    },
    [upsert],
  );

  const update = useCallback(
    async (payload: UpdateScheduledJobPayload) => {
      const saved = await updateScheduledJob(payload);
      upsert(saved);
      return saved;
    },
    [upsert],
  );

  const remove = useCallback(async (taskId: string) => {
    await deleteScheduledJob(taskId);
    setTasks((current) => current.filter((task) => task.id !== taskId));
  }, []);

  const run = useCallback(
    async (taskId: string) => {
      const saved = await triggerScheduledJob(taskId);
      upsert(saved);
      return saved;
    },
    [upsert],
  );

  const toggle = useCallback(
    async (task: ScheduledJob) => {
      const saved = await updateScheduledJob({ jobId: task.id, enabled: !task.enabled });
      upsert(saved);
      return saved;
    },
    [upsert],
  );

  return {
    tasks,
    blueprints,
    isLoading,
    error,
    refresh,
    create,
    update,
    remove,
    run,
    toggle,
  };
}

export type TasksState = ReturnType<typeof useTasks>;
