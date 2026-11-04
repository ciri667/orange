import { useCallback, useEffect, useRef, useState } from "react";
import type { ScheduledJob } from "../shared/types";
import {
  REMINDER_INBOX_STORAGE_KEY,
  REMINDER_TOAST_TTL_MS,
  dismissReminder,
  dismissRemindersForJob,
  mergeReminderNotice,
  parseReminderNotices,
  reminderNoticeFromJob,
  serializeReminderNotices,
  type ReminderNotice,
} from "./reminderInbox";

/**
 * 提醒收件箱 + 应用内 Toast 的数据源。
 *
 * 两者刻意分开：Toast 是「此刻正在屏幕上的提示」，会自动收起；
 * 收件箱是「用户还没处理过的提醒」，只有用户主动查看或点「知道了」才消失。
 * 窗口不在前台时后端会额外发系统通知，所以这里不需要关心窗口焦点。
 */
export function useReminderInbox() {
  const [notices, setNotices] = useState<ReminderNotice[]>(() => readStoredNotices());
  const [toasts, setToasts] = useState<ReminderNotice[]>([]);
  /** Toast 自动收起的定时器；卸载时统一清掉，避免对已卸载组件 setState。 */
  const toastTimers = useRef(new Map<string, number>());

  useEffect(() => {
    writeStoredNotices(notices);
  }, [notices]);

  useEffect(() => {
    const timers = toastTimers.current;
    return () => {
      for (const timer of timers.values()) {
        window.clearTimeout(timer);
      }
      timers.clear();
    };
  }, []);

  /** 收起一条 Toast，但保留收件箱里的未读。 */
  const dismissToast = useCallback((id: string) => {
    const timer = toastTimers.current.get(id);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      toastTimers.current.delete(id);
    }
    setToasts((current) => current.filter((item) => item.id !== id));
  }, []);

  /**
   * 登记一条新提醒：同时进入收件箱和 Toast。
   *
   * 非提醒任务或状态不是 reminded 时返回 null，调用方可以据此跳过后续提示。
   */
  const push = useCallback((job: ScheduledJob): ReminderNotice | null => {
    const notice = reminderNoticeFromJob(job);
    if (!notice) {
      return null;
    }
    setNotices((current) => mergeReminderNotice(current, notice));
    setToasts((current) => mergeReminderNotice(current, notice, 3));

    const existing = toastTimers.current.get(notice.id);
    if (existing !== undefined) {
      window.clearTimeout(existing);
    }
    toastTimers.current.set(
      notice.id,
      window.setTimeout(() => {
        toastTimers.current.delete(notice.id);
        setToasts((current) => current.filter((item) => item.id !== notice.id));
      }, REMINDER_TOAST_TTL_MS),
    );
    return notice;
  }, []);

  /** 用户确认某条提醒：Toast 和未读一起清掉。 */
  const dismiss = useCallback(
    (id: string) => {
      dismissToast(id);
      setNotices((current) => dismissReminder(current, id));
    },
    [dismissToast],
  );

  /** 打开某个任务时清掉它的全部未读，避免角标长期挂着。 */
  const dismissJob = useCallback((jobId: string) => {
    setNotices((current) => dismissRemindersForJob(current, jobId));
    setToasts((current) => current.filter((item) => item.jobId !== jobId));
  }, []);

  /** 全部已读：清空收件箱和屏幕上的 Toast。 */
  const clear = useCallback(() => {
    for (const timer of toastTimers.current.values()) {
      window.clearTimeout(timer);
    }
    toastTimers.current.clear();
    setNotices([]);
    setToasts([]);
  }, []);

  return { notices, toasts, unreadCount: notices.length, push, dismiss, dismissToast, dismissJob, clear };
}

export type ReminderInboxState = ReturnType<typeof useReminderInbox>;

/** 读取持久化收件箱；隐私模式或禁用存储时退化为空。 */
function readStoredNotices(): ReminderNotice[] {
  try {
    return parseReminderNotices(window.localStorage.getItem(REMINDER_INBOX_STORAGE_KEY));
  } catch {
    return [];
  }
}

/** 写入持久化收件箱；失败只影响下次启动的未读恢复，不打断当前交互。 */
function writeStoredNotices(notices: ReminderNotice[]) {
  try {
    window.localStorage.setItem(REMINDER_INBOX_STORAGE_KEY, serializeReminderNotices(notices));
  } catch {
    // 存储不可用时保留内存态即可。
  }
}
