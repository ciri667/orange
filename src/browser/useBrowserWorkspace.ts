import { useCallback, useEffect, useRef, useState } from "react";
import type { BrowserActivity, BrowserApprovalRequest, BrowserFrame, BrowserReceipt, BrowserState } from "../shared/types";
import { browserSetScreencast, browserShowTab, loadBrowserStatus, subscribeBrowserActivity, subscribeBrowserApprovals, subscribeBrowserFrames, subscribeBrowserReceipts, subscribeBrowserState } from "../shared/tauriApi";

/** 常驻浏览器状态层；切换到会话或终端后仍接收新网页和审批。 */
export function useBrowserWorkspace(selectedTabId: string | null, visible: boolean) {
  const [state, setState] = useState<BrowserState | null>(null);
  const [frame, setFrame] = useState<BrowserFrame | null>(null);
  const [activity, setActivity] = useState<BrowserActivity | null>(null);
  const [receipt, setReceipt] = useState<BrowserReceipt | null>(null);
  const [approval, setApproval] = useState<BrowserApprovalRequest | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [screencast, setScreencast] = useState(true);
  const viewQueue = useRef(Promise.resolve());

  const runAction = useCallback(async (action: () => Promise<BrowserState | void>) => {
    setBusy(true);
    setError(null);
    try { const next = await action(); if (next) setState(next); return next; }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); throw cause; }
    finally { setBusy(false); }
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let activityTimer: ReturnType<typeof setTimeout>;
    const stops: (() => void)[] = [];
    const track = (promise: Promise<() => void>) => { void promise.then((stop) => disposed ? stop() : stops.push(stop)).catch(() => undefined); };
    track(subscribeBrowserState((next) => { if (!disposed) setState(next); }));
    track(subscribeBrowserFrames((next) => { if (!disposed) setFrame(next); }));
    track(subscribeBrowserReceipts((next) => { if (!disposed) setReceipt(next); }));
    track(subscribeBrowserApprovals((next) => { if (!disposed) setApproval(next); }));
    track(subscribeBrowserActivity((next) => {
      if (disposed) return;
      setActivity(next);
      clearTimeout(activityTimer);
      activityTimer = setTimeout(() => setActivity(null), 4000);
    }));
    const refresh = async () => {
      try { const next = await loadBrowserStatus(); if (!disposed) setState(next); }
      catch { /* 状态读取失败留待下一次；操作失败在内容区展示。 */ }
      finally { if (!disposed) timer = setTimeout(() => void refresh(), 2000); }
    };
    void refresh();
    return () => { disposed = true; clearTimeout(timer); clearTimeout(activityTimer); stops.forEach((stop) => stop()); void browserSetScreencast(false).catch(() => undefined); };
  }, []);

  useEffect(() => {
    let disposed = false;
    // 串行切换画面；旧的异步选择不能在用户切走之后再次开启旧网页的流。
    viewQueue.current = viewQueue.current.catch(() => undefined).then(async () => {
      if (disposed) return;
      if (visible && selectedTabId && state?.tabs.some((tab) => tab.tabId === selectedTabId)) {
        await browserShowTab(selectedTabId);
        if (!disposed) await browserSetScreencast(screencast);
      } else { await browserSetScreencast(false); }
    }).catch((cause) => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; };
  }, [selectedTabId, visible, state?.available, screencast]);

  return { state, frame, activity, receipt, approval, setApproval, error, busy, screencast, setScreencast, runAction };
}
export type BrowserWorkspace = ReturnType<typeof useBrowserWorkspace>;
