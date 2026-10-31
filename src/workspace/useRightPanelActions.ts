import { useEffect, useRef } from "react";
import { browserCloseTab, browserCreateTab, browserUserNavigate, isTauriRuntime } from "../shared/tauriApi";
import type { ConfirmDialogConfig } from "../shared/ConfirmDialog";
import type { RightPanelTab, WorkspaceSnapshot } from "../shared/types";
import type { ExternalLinkRequest } from "../shared/externalLink";
import { logWarn } from "../shared/logger";
import type { BrowserWorkspace } from "../browser/useBrowserWorkspace";
import { resolveExternalLinkTarget } from "../browser/linkRouting";
import type { Terminals } from "../terminal/useTerminals";
import type { RightPanelTabs } from "./useRightPanelTabs";
import { browserTabToFollow, rightTabId } from "./rightPanelModel";

interface Options {
  panel: RightPanelTabs;
  browser: BrowserWorkspace;
  terminals: Terminals;
  snapshot: WorkspaceSnapshot | null;
  browserEnabled: boolean;
  /** 设置里的“网页链接默认在内置浏览器打开”；关闭后链接回到系统默认浏览器。 */
  openLinksInEmbedded: boolean;
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
    const follow = browserTabToFollow(panel.stateRef.current, browser.state, browser.pendingTabId);
    if (follow) panel.activateTab(follow);
  }, [options.browser.state, options.browser.pendingTabId]);

  const select = (tab: RightPanelTab) => {
    const { panel, browser, reveal, selectSession, notice } = current.current;
    if (tab.kind === "browser" && !tab.id.startsWith("pending-")) browser.selectTab(tab.id);
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
  /**
   * 网页链接点击的落点：默认在内置浏览器新开页签，无法使用时回退系统默认浏览器。
   *
   * 返回 true 表示已经由内置浏览器接手；返回 false 由调用方（MarkdownLink）走系统打开。
   */
  const openExternalLink = async ({ href, isWebPage }: ExternalLinkRequest): Promise<boolean> => {
    const { panel, browser, browserEnabled, openLinksInEmbedded, reveal, notice } = current.current;
    const decision = resolveExternalLinkTarget({
      isWebPage,
      // Ctrl / Cmd 点击已在 MarkdownLink 里判定过，这里只处理默认路径。
      bypassEmbeddedBrowser: false,
      isDesktopRuntime: isTauriRuntime(),
      browserEnabled,
      openLinksInEmbedded,
      // 状态还没读到时先按“可用”尝试；真没安装会在创建页签时失败，同样回退系统浏览器。
      chromiumInstalled: browser.state ? browser.state.chromiumInstalled : true,
    });
    if (decision.notice) notice(decision.notice);
    if (decision.target === "system") return false;

    // 先放占位页：首次点击时创建页签会顺手启动 Chromium，可能有一两秒延迟。
    const pendingId = `pending-${crypto.randomUUID()}`;
    panel.openTab({ kind: "browser", id: pendingId });
    reveal();
    const before = new Set(browser.state?.tabs.map((tab) => tab.tabId));
    try {
      const next = await browser.runAction(browserCreateTab);
      const created = next?.tabs.find((tab) => !before.has(tab.tabId));
      if (!created) throw new Error("内置浏览器没有返回新建的页签。");
      // 用户在创建期间关掉占位页：回收刚创建的页签，不抢焦点。
      if (!panel.stateRef.current.tabs.some((tab) => tab.kind === "browser" && tab.id === pendingId)) {
        await browser.runAction(() => browserCloseTab(created.tabId));
        return true;
      }
      panel.replaceTab(`browser:${pendingId}`, { kind: "browser", id: created.tabId });
      await browser.runAction(() => browserUserNavigate(href, created.tabId));
      return true;
    } catch (cause) {
      // 失败时清掉占位页，避免面板停在“网页尚未打开”；随后由调用方回退系统浏览器。
      if (panel.stateRef.current.tabs.some((tab) => tab.kind === "browser" && tab.id === pendingId)) panel.removeTab(`browser:${pendingId}`);
      logWarn("内置浏览器打开链接失败，已回退系统默认浏览器。", {
        category: "frontend",
        event: "open_external_link_in_embedded_browser",
        status: "failed",
        error: cause,
      });
      notice("内置浏览器没能打开这个链接，已改用系统默认浏览器打开。");
      return false;
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
  return { select, create, close, openExternalLink, retryBrowser: (id: string) => void createBrowser(id) };
}
