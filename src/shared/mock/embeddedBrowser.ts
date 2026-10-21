import { browserMock } from "./browser";
import type { BrowserState, ChromiumInstallStatus } from "../types";

/** Web 开发态没有桌面 Chromium 会话，保留设置值并明确返回不可用状态。 */
export function loadMockEmbeddedBrowserStatus(): BrowserState {
  return {
    available: false,
    enabled: browserMock.userSettings.browser.enabled,
    chromiumInstalled: false,
    launchError: null,
    tabs: [],
    activeTabId: null,
    visibleTabId: null,
    control: { running: false, sessionId: null, status: "idle", revision: 0, elapsedMs: 0 },
  };
}

/** Web 开发态不会下载内核。 */
export function loadMockChromiumStatus(): ChromiumInstallStatus {
  return { installed: false, version: null, downloading: false, phase: "idle", downloaded: 0, total: null, error: null };
}
