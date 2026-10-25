import { useEffect, useRef } from "react";
import { browserCloseTab, browserCreateTab } from "../shared/tauriApi";
import type { ConfirmDialogConfig } from "../shared/ConfirmDialog";
import type { RightPanelTab, WorkspaceSnapshot } from "../shared/types";
import type { BrowserWorkspace } from "../browser/useBrowserWorkspace";
import type { Terminals } from "../terminal/useTerminals";
import type { RightPanelTabs } from "./useRightPanelTabs";
import { rightTabId } from "./rightPanelModel";

interface Options {
  panel: RightPanelTabs;
  browser: BrowserWorkspace;
  terminals: Terminals;
  snapshot: WorkspaceSnapshot | null;
  browserEnabled: boolean;
  reveal: () => void;
  showSettings: () => void;
  selectSession: (sessionId: string) => Promise<void>;
  createSession: () => Promise<void>;
  confirm: (config: ConfirmDialogConfig, action: () => Promise<void> | void) => void;
  notice: (message: string) => void;
}

/** 用户页签动作与后台网页发现分开处理，后台事件只追加内容不打开栏或抢焦点。 */
export function useRightPanelActions(options: Options) {
  const current = useRef(options);
  current.current = options;
  const browserCreating = useRef(new Set<string>());

  useEffect(() => {
    const { panel, browser } = current.current;
    if (!browser.state) return;
    for (const tab of browser.state.tabs) panel.openTab({ kind: "browser", id: tab.tabId }, false);
    for (const tab of panel.stateRef.current.tabs) {
      if (tab.kind === "browser" && !tab.id.startsWith("pending-") && !browser.state.tabs.some((page) => page.tabId === tab.id)) panel.removeTab(rightTabId(tab));
    }
  }, [options.browser.state]);

  const select = (tab: RightPanelTab) => {
    const { panel, reveal, selectSession, notice } = current.current;
    panel.activateTab(rightTabId(tab));
    reveal();
    if (tab.kind === "session") void selectSession(tab.id).catch((cause) => notice(String(cause)));
  };
  const createBrowser = async (pendingId = `pending-${crypto.randomUUID()}`) => {
    const { panel, browserEnabled, showSettings, reveal, browser } = current.current;
    if (!browserEnabled) { showSettings(); return; }
    if (browserCreating.current.has(pendingId)) return;
    browserCreating.current.add(pendingId);
    panel.openTab({ kind: "browser", id: pendingId });
    reveal();
    const before = new Set(browser.state?.tabs.map((tab) => tab.tabId));
    try {
      const next = await browser.runAction(browserCreateTab);
      if (next) {
        const created = next.tabs.find((tab) => !before.has(tab.tabId));
        if (created) {
          // 创建结果迟到时只替换占位页，不重新激活；占位页已关闭则回收刚创建的网页。
          if (current.current.panel.stateRef.current.tabs.some((tab) => tab.kind === "browser" && tab.id === pendingId)) panel.replaceTab(`browser:${pendingId}`, { kind: "browser", id: created.tabId });
          else await browser.runAction(() => browserCloseTab(created.tabId));
        }
      }
    } catch { /* 错误在同一浏览器占位页展示，可原地重试。 */ }
    finally { browserCreating.current.delete(pendingId); }
  };
  const create = (kind: RightPanelTab["kind"]) => {
    const { snapshot, terminals, panel, reveal, createSession } = current.current;
    if (kind === "session") { reveal(); void createSession(); }
    else if (kind === "browser") void createBrowser();
    else if (snapshot) {
      const id = terminals.create(snapshot.activeKnowledgeBaseId);
      panel.openTab({ kind: "terminal", id });
      reveal();
    }
  };
  const close = (tab: RightPanelTab) => {
    const { panel, terminals, browser, confirm, selectSession, notice } = current.current;
    const perform = async () => {
      try {
        const wasActive = panel.stateRef.current.activeId === rightTabId(tab);
        if (tab.kind === "terminal") await terminals.close(tab.id);
        if (tab.kind === "browser" && !tab.id.startsWith("pending-")) await browser.runAction(() => browserCloseTab(tab.id));
        const next = panel.removeTab(rightTabId(tab));
        const active = next.tabs.find((item) => rightTabId(item) === next.activeId);
        if (wasActive && active?.kind === "session") await selectSession(active.id);
      } catch (cause) { notice(String(cause)); }
    };
    const terminal = tab.kind === "terminal" ? terminals.get(tab.id) : undefined;
    if (terminal?.status === "running" || terminal?.status === "starting") confirm({ title: "关闭终端", message: "关闭会结束这个终端及其中运行的程序。", confirmLabel: "结束并关闭", tone: "danger" }, perform);
    else void perform();
  };
  return { select, create, close, retryBrowser: (id: string) => void createBrowser(id) };
}
