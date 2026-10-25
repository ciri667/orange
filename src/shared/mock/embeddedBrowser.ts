import { browserMock } from "./browser";
import type { BrowserState, ChromiumInstallStatus } from "../types";

/** 开发态仅模拟页签，不连接外部网页或假装拥有桌面内核。 */
const tabs: BrowserState["tabs"] = [];
let visibleTabId: string | null = null;
export function createMockBrowserTab() {
  const tabId = crypto.randomUUID();
  tabs.push({ tabId, title: "空白页", url: "about:blank", active: false, visible: true });
  visibleTabId = tabId;
  return loadMockEmbeddedBrowserStatus();
}
export function showMockBrowserTab(tabId: string) { visibleTabId = tabId; return loadMockEmbeddedBrowserStatus(); }
export function closeMockBrowserTab(tabId: string) {
  const index = tabs.findIndex((tab) => tab.tabId === tabId);
  if (index >= 0) tabs.splice(index, 1);
  if (visibleTabId === tabId) visibleTabId = null;
  return loadMockEmbeddedBrowserStatus();
}
export function navigateMockBrowserTab(url: string, tabId?: string) {
  const tab = tabs.find((tab) => tab.tabId === (tabId ?? visibleTabId));
  if (tab) { tab.url = url; tab.title = url; }
  return loadMockEmbeddedBrowserStatus();
}

/** Web 开发态没有桌面 Chromium 会话，保留设置值并明确返回不可用状态。 */
export function loadMockEmbeddedBrowserStatus(): BrowserState {
  return {
    available: false,
    enabled: browserMock.userSettings.browser.enabled,
    chromiumInstalled: false,
    launchError: null,
    tabs: tabs.map((tab) => ({ ...tab, visible: tab.tabId === visibleTabId })),
    activeTabId: null,
    visibleTabId,
    control: { running: false, sessionId: null, status: "idle", revision: 0, elapsedMs: 0 },
  };
}

/** Web 开发态不会下载内核。 */
export function loadMockChromiumStatus(): ChromiumInstallStatus {
  return { installed: false, version: null, downloading: false, phase: "idle", downloaded: 0, total: null, error: null };
}
