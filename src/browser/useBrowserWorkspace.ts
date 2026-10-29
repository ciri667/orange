import { useCallback, useEffect, useRef, useState } from "react";
import type { BrowserActivity, BrowserFrame, BrowserReceipt, BrowserState } from "../shared/types";
import { browserActivateTab, browserSetScreencast, browserShowTab, loadBrowserStatus, subscribeBrowserActivity, subscribeBrowserFrames, subscribeBrowserReceipts, subscribeBrowserState } from "../shared/tauriApi";

/** 常驻浏览器状态层；切换到会话或终端后仍接收新网页、画面、活动提示和操作凭证。 */
export function useBrowserWorkspace(selectedTabId: string | null, visible: boolean) {
  const [state, setState] = useState<BrowserState | null>(null);
  const [frame, setFrame] = useState<BrowserFrame | null>(null);
  const [activity, setActivity] = useState<BrowserActivity | null>(null);
  const [receipt, setReceipt] = useState<BrowserReceipt | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [screencast, setScreencast] = useState(true);
  const viewQueue = useRef(Promise.resolve());
  const pendingView = useRef<string | null>(null);
  const [pendingTabId, setPendingTabId] = useState<string | null>(null);
  const stateRef = useRef<BrowserState | null>(null);
  const applyState = useCallback((next: BrowserState) => {
    // 接管前发出的查询可能较晚返回；不能把较旧的租约恢复到界面上。
    if (stateRef.current && next.control.revision < stateRef.current.control.revision) return;
    stateRef.current = next;
    setState(next);
  }, []);

  const runAction = useCallback(async (action: () => Promise<BrowserState | void>) => {
    setBusy(true);
    setError(null);
    try { const next = await action(); if (next) applyState(next); return next; }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); throw cause; }
    finally { setBusy(false); }
  }, [applyState]);

  /** 用户主动切页先接管；与 Agent 画面跟随分开，避免跟随操作反而取消 Agent。 */
  const selectTab = useCallback((tabId: string) => {
    pendingView.current = tabId;
    setPendingTabId(tabId);
    viewQueue.current = viewQueue.current.catch(() => undefined).then(async () => {
      try { await runAction(() => browserActivateTab(tabId)); }
      finally {
        if (pendingView.current === tabId) {
          pendingView.current = null;
          setPendingTabId(null);
        }
      }
    }).catch(() => undefined);
  }, [runAction]);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let activityTimer: ReturnType<typeof setTimeout>;
    const stops: (() => void)[] = [];
    const track = (promise: Promise<() => void>) => { void promise.then((stop) => disposed ? stop() : stops.push(stop)).catch(() => undefined); };
    track(subscribeBrowserState((next) => { if (!disposed) applyState(next); }));
    track(subscribeBrowserFrames((next) => { if (!disposed) setFrame(next); }));
    track(subscribeBrowserReceipts((next) => { if (!disposed) setReceipt(next); }));
    track(subscribeBrowserActivity((next) => {
      if (disposed) return;
      setActivity(next);
      clearTimeout(activityTimer);
      activityTimer = setTimeout(() => setActivity(null), 4000);
    }));
    const refresh = async () => {
      try { const next = await loadBrowserStatus(); if (!disposed) applyState(next); }
      catch { /* 状态读取失败留待下一次；操作失败在内容区展示。 */ }
      finally { if (!disposed) timer = setTimeout(() => void refresh(), 5000); }
    };
    void refresh();
    return () => { disposed = true; clearTimeout(timer); clearTimeout(activityTimer); stops.forEach((stop) => stop()); void browserSetScreencast(false).catch(() => undefined); };
  }, [applyState]);

  useEffect(() => {
    let disposed = false;
    // 串行切换画面；旧的异步选择不能在用户切走之后再次开启旧网页的流。
    viewQueue.current = viewQueue.current.catch(() => undefined).then(async () => {
      if (disposed || pendingView.current) return;
      if (visible && selectedTabId && state?.tabs.some((tab) => tab.tabId === selectedTabId)) {
        await browserShowTab(selectedTabId);
        if (!disposed) await browserSetScreencast(screencast);
      } else { await browserSetScreencast(false); }
    }).catch((cause) => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; };
  }, [selectedTabId, visible, state?.available, screencast, pendingTabId]);

  return { state, frame, activity, receipt, error, busy, screencast, setScreencast, runAction, selectTab, pendingTabId };
}
export type BrowserWorkspace = ReturnType<typeof useBrowserWorkspace>;
