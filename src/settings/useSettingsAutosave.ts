import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";

export type SettingsSaveStatus = "idle" | "pending" | "saving" | "saved" | "error";
export type SettingsDraftChange<T> = (next: SetStateAction<T>, defer?: boolean) => void;

/** 设置草稿按用户操作保存；文字输入可延迟到失焦，连续操作串行写入，避免旧请求覆盖新值。 */
export function useSettingsAutosave<T>(value: T, save: (next: T) => Promise<T>) {
  const [draft, setDraft] = useState(value);
  const [status, setStatus] = useState<SettingsSaveStatus>("idle");
  const [error, setError] = useState("");
  const draftRef = useRef(value);
  const savedRef = useRef(value);
  const revisionRef = useRef(0);
  const dirtyRef = useRef(false);
  const readyRef = useRef(false);
  const runningRef = useRef<Promise<boolean> | null>(null);
  const saveRef = useRef(save);
  saveRef.current = save;

  /** 接收后端归一化结果或初始化草稿，不把外部同步当成新的用户修改。 */
  const replace = useCallback((next: SetStateAction<T>) => {
    const resolved = typeof next === "function" ? (next as (current: T) => T)(draftRef.current) : next;
    draftRef.current = resolved;
    savedRef.current = resolved;
    dirtyRef.current = false;
    readyRef.current = false;
    setDraft(resolved);
  }, []);

  useEffect(() => {
    // 保存中的回传值可能属于旧版本；由保存队列接收结果，不能覆盖用户继续输入的内容。
    if (!dirtyRef.current && !runningRef.current) replace(value);
  }, [value, replace]);

  /** 关闭面板、切换分区或完成输入前等待全部修改落盘；失败保留草稿供修正或重试。 */
  const flush = useCallback((force = false): Promise<boolean> => {
    // 启动 IM 等明确动作需要连同预填的默认范围一起保存，即使开关值没有变化。
    if (force) dirtyRef.current = true;
    readyRef.current = true;
    if (runningRef.current) return runningRef.current;
    if (!dirtyRef.current) return Promise.resolve(true);

    // 微任务合并同一次点击引发的多次更新，并先登记请求，防止 blur/click 同时保存。
    const operation = Promise.resolve().then(async () => {
      while (dirtyRef.current && readyRef.current) {
        const next = draftRef.current;
        const revision = revisionRef.current;
        setStatus("saving");
        setError("");
        try {
          const saved = await saveRef.current(next);
          savedRef.current = saved;
          if (revisionRef.current === revision) {
            draftRef.current = saved;
            dirtyRef.current = false;
            setDraft(saved);
          }
        } catch (cause) {
          setError(cause instanceof Error ? cause.message : String(cause));
          setStatus("error");
          return false;
        }
      }
      setStatus(dirtyRef.current ? "pending" : "saved");
      return !dirtyRef.current;
    }).finally(() => { runningRef.current = null; });
    runningRef.current = operation;
    return operation;
  }, []);

  const update: SettingsDraftChange<T> = useCallback((next, defer = false) => {
    const resolved = typeof next === "function" ? (next as (current: T) => T)(draftRef.current) : next;
    if (JSON.stringify(resolved) === JSON.stringify(draftRef.current)) return;
    draftRef.current = resolved;
    revisionRef.current += 1;
    // 即使用户改回原值，也要排在尚未完成的旧写入后再次保存。
    dirtyRef.current = runningRef.current !== null || JSON.stringify(resolved) !== JSON.stringify(savedRef.current);
    readyRef.current = !defer;
    setDraft(resolved);
    setError("");
    setStatus(dirtyRef.current ? "pending" : "saved");
    if (!defer) void flush();
  }, [flush]);

  return { draft, update, replace, flush, status, error };
}
