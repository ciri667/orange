import type { BrowserState, RightPanelTab } from "../shared/types";

/** 页签状态独立于编辑器和领域快照，后台回合不能改变正在查看的内容。 */
export interface RightPanelState {
  tabs: RightPanelTab[];
  activeId: string | null;
}

export const rightTabId = (tab: RightPanelTab) => `${tab.kind}:${tab.id}`;

/** 仅已在查看网页时跟随 Agent；会话、终端、创建占位页和用户正在切换的目标保持原选择。 */
export function browserTabToFollow(state: RightPanelState, browser: BrowserState, pendingTabId: string | null): string | null {
  const selected = state.tabs.find((tab) => rightTabId(tab) === state.activeId);
  const visible = browser.visibleTabId;
  if (pendingTabId || selected?.kind !== "browser" || selected.id.startsWith("pending-")
    || !browser.control.sessionId || !visible || selected.id === visible
    || !browser.tabs.some((tab) => tab.tabId === visible)) return null;
  return `browser:${visible}`;
}

/** 打开稳定引用时去重；后台发现网页可以仅追加而不激活。 */
export function openRightTab(state: RightPanelState, tab: RightPanelTab, activate = true): RightPanelState {
  const id = rightTabId(tab);
  return {
    tabs: state.tabs.some((item) => rightTabId(item) === id) ? state.tabs : [...state.tabs, tab],
    activeId: activate ? id : state.activeId,
  };
}

/** 关闭当前页签优先选择右邻，最后一个关闭后没有活动内容。 */
export function closeRightTab(state: RightPanelState, id: string): RightPanelState {
  const index = state.tabs.findIndex((tab) => rightTabId(tab) === id);
  if (index < 0) return state;
  const tabs = state.tabs.filter((tab) => rightTabId(tab) !== id);
  const neighbor = tabs[index] ?? tabs[index - 1];
  return { tabs, activeId: state.activeId === id ? (neighbor ? rightTabId(neighbor) : null) : state.activeId };
}

/** 拖拽只改变展示顺序，稳定内容 ID 和活动页签保持原样。 */
export function reorderRightTabs(state: RightPanelState, source: string, target: string): RightPanelState {
  const from = state.tabs.findIndex((tab) => rightTabId(tab) === source);
  const to = state.tabs.findIndex((tab) => rightTabId(tab) === target);
  if (from < 0 || to < 0 || from === to) return state;
  const tabs = [...state.tabs];
  tabs.splice(to, 0, tabs.splice(from, 1)[0]);
  return { ...state, tabs };
}

/** 仅恢复存在的会话；损坏记录降级为空，缺少记录才执行旧版迁移。 */
export function restoreRightTabs(raw: string | null, sessionIds: string[], legacySessionId: string): RightPanelState {
  if (raw === null) return sessionIds.includes(legacySessionId) ? { tabs: [{ kind: "session", id: legacySessionId }], activeId: `session:${legacySessionId}` } : { tabs: [], activeId: null };
  try {
    const saved = JSON.parse(raw);
    if (saved.version !== 1 || !Array.isArray(saved.sessionIds)) return { tabs: [], activeId: null };
    const ids = [...new Set<string>(saved.sessionIds.filter((id: unknown) => typeof id === "string" && sessionIds.includes(id)))];
    const active = ids.includes(saved.activeSessionId) ? saved.activeSessionId : ids[0];
    return { tabs: ids.map((id) => ({ kind: "session", id })), activeId: active ? `session:${active}` : null };
  } catch { return { tabs: [], activeId: null }; }
}
