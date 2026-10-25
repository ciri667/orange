import { useCallback, useEffect, useRef, useState } from "react";
import type { RightPanelTab, WorkspaceSnapshot } from "../shared/types";
import { closeRightTab, openRightTab, reorderRightTabs, restoreRightTabs, rightTabId, type RightPanelState } from "./rightPanelModel";

const STORAGE_KEY = "orange-workspace-right-panel-v1";

/** 管理统一内容页签；本机记录只包含会话引用，不启动终端或加载网页。 */
export function useRightPanelTabs(snapshot: WorkspaceSnapshot | null, setSnapshot: (update: (current: WorkspaceSnapshot | null) => WorkspaceSnapshot | null) => void, setOpen: (open: boolean) => void) {
  const [state, setState] = useState<RightPanelState>({ tabs: [], activeId: null });
  const initialized = useRef(false);
  const [ready, setReady] = useState(false);
  const stateRef = useRef(state);
  stateRef.current = state;
  const lastSessionId = useRef<string | null>(null);
  const focusRevision = useRef(0);
  const update = useCallback((change: (state: RightPanelState) => RightPanelState) => {
    const next = change(stateRef.current);
    stateRef.current = next;
    setState(next);
    return next;
  }, []);

  useEffect(() => {
    if (!snapshot || initialized.current) return;
    initialized.current = true;
    let raw: string | null = null;
    try { raw = localStorage.getItem(STORAGE_KEY); } catch { /* WebView 存储不可用时仍可使用页签。 */ }
    const next = restoreRightTabs(raw, snapshot.sessions.map((session) => session.id), snapshot.activeSessionId);
    setState(next);
    stateRef.current = next;
    setReady(true);
    if (!next.tabs.length) setOpen(false);
    const id = next.tabs.find((tab) => rightTabId(tab) === next.activeId)?.id;
    if (id) setSnapshot((current) => current ? { ...current, activeSessionId: id } : current);
  }, [snapshot, setOpen, setSnapshot]);

  useEffect(() => {
    if (!initialized.current || !snapshot) return;
    // 删除会话后清理引用；归档会话仍可以保持打开。
    const next = update((current) => {
      let next = current;
      for (const tab of current.tabs) if (tab.kind === "session" && !snapshot.sessions.some((session) => session.id === tab.id)) next = closeRightTab(next, rightTabId(tab));
      return next;
    });
    if (!next.tabs.length) setOpen(false);
  }, [snapshot?.sessions]);

  useEffect(() => {
    if (!ready) return;
    const activeSession = state.tabs.find((tab) => rightTabId(tab) === state.activeId && tab.kind === "session");
    if (activeSession) lastSessionId.current = activeSession.id;
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify({ version: 1, sessionIds: state.tabs.filter((tab) => tab.kind === "session").map((tab) => tab.id), activeSessionId: lastSessionId.current }));
    } catch { /* 持久化失败不能阻止当前内容运行。 */ }
  }, [state, ready]);

  const openTab = useCallback((tab: RightPanelTab, activate = true) => { if (activate) ++focusRevision.current; update((current) => openRightTab(current, tab, activate)); }, [update]);
  const activateTab = useCallback((id: string) => { ++focusRevision.current; update((current) => current.tabs.some((tab) => rightTabId(tab) === id) ? { ...current, activeId: id } : current); }, [update]);
  const removeTab = useCallback((id: string) => {
    const next = closeRightTab(stateRef.current, id);
    stateRef.current = next;
    setState(next);
    if (!next.tabs.length) setOpen(false);
    return next;
  }, [setOpen]);
  const replaceTab = useCallback((id: string, replacement: RightPanelTab) => update((current) => {
    if (!current.tabs.some((tab) => rightTabId(tab) === id)) return current;
    const tabs = current.tabs.flatMap((tab) => rightTabId(tab) === id ? [replacement] : rightTabId(tab) === rightTabId(replacement) ? [] : [tab]);
    return { tabs, activeId: current.activeId === id ? rightTabId(replacement) : current.activeId };
  }), [update]);
  return { ...state, stateRef, openTab, activateTab, removeTab, replaceTab, captureFocus: () => { const revision = ++focusRevision.current; return () => revision === focusRevision.current; }, reorder: (source: string, target: string) => update((current) => reorderRightTabs(current, source, target)) };
}
export type RightPanelTabs = ReturnType<typeof useRightPanelTabs>;
