import { listen } from "@tauri-apps/api/event";
import { invokeLogged, isTauriRuntime } from "./runtime";
import { loadMockChromiumStatus, loadMockEmbeddedBrowserStatus, createMockBrowserTab, showMockBrowserTab, closeMockBrowserTab, navigateMockBrowserTab } from "../mock/embeddedBrowser";
import type {
  BrowserApprovalDecision,
  BrowserApprovalRequest,
  BrowserActivity,
  BrowserFrame,
  BrowserReceipt,
  BrowserState,
  BrowserUserInput,
  ChromiumInstallStatus,
} from "../types";

/** 与 Rust 侧事件名保持一致。 */
const BROWSER_STATE_EVENT = "browser-state-changed";
const BROWSER_FRAME_EVENT = "browser-frame";
const BROWSER_ACTIVITY_EVENT = "browser-activity";
const BROWSER_RECEIPT_EVENT = "browser-receipt";
const BROWSER_APPROVAL_EVENT = "browser-operation-approval";
const BROWSER_DOWNLOAD_PROGRESS_EVENT = "browser-download-progress";

/** 读取浏览器整体状态；面板和设置页共用。 */
export async function loadBrowserStatus(): Promise<BrowserState> {
  if (!isTauriRuntime()) {
    return loadMockEmbeddedBrowserStatus();
  }
  return invokeLogged<BrowserState>("browser_status");
}

/** 面板明确启动或重试浏览器。 */
export async function browserStart(): Promise<BrowserState> {
  if (!isTauriRuntime()) return loadBrowserStatus();
  return invokeLogged<BrowserState>("browser_start");
}

export async function browserCreateTab(): Promise<BrowserState> {
  if (!isTauriRuntime()) return createMockBrowserTab();
  return invokeLogged<BrowserState>("browser_create_tab");
}

/** 查看页签不撤销 Agent 的浏览器控制租约。 */
export async function browserShowTab(tabId: string): Promise<BrowserState> {
  if (!isTauriRuntime()) return showMockBrowserTab(tabId);
  return invokeLogged<BrowserState>("browser_show_tab", { tabId });
}

export async function browserActivateTab(tabId: string): Promise<BrowserState> {
  if (!isTauriRuntime()) return loadBrowserStatus();
  return invokeLogged<BrowserState>("browser_activate_tab", { tabId });
}

/** 用户输入不随 IPC 调用写入日志或 Agent 上下文。 */
export async function browserUserInput(tabId: string, input: BrowserUserInput): Promise<void> {
  if (!isTauriRuntime()) return;
  return invokeLogged<void>("browser_user_input", { tabId, input });
}

/** 用户在设置里开启/关闭浏览器功能；关闭时后端会停掉浏览器进程。 */
export async function setBrowserEnabled(enabled: boolean): Promise<BrowserState> {
  if (!isTauriRuntime()) {
    return loadBrowserStatus();
  }
  return invokeLogged<BrowserState>("set_browser_enabled", { enabled });
}

/** 首次启用时下载固定版本 Chromium；绝不静默下载。 */
export async function downloadBrowserChromium(): Promise<ChromiumInstallStatus> {
  if (!isTauriRuntime()) {
    throw new Error("浏览器内核下载仅在橘记桌面端可用，请在桌面端操作。");
  }
  return invokeLogged<ChromiumInstallStatus>("download_browser_chromium");
}

/** 查询 Chromium 安装状态。 */
export async function loadBrowserChromiumStatus(): Promise<ChromiumInstallStatus> {
  if (!isTauriRuntime()) {
    return loadMockChromiumStatus();
  }
  return invokeLogged<ChromiumInstallStatus>("browser_chromium_status");
}

/** 用户在面板地址栏手动导航。 */
export async function browserUserNavigate(url: string, tabId?: string): Promise<BrowserState> {
  if (!isTauriRuntime()) {
    return navigateMockBrowserTab(url, tabId);
  }
  return invokeLogged<BrowserState>("browser_user_navigate", { url, tabId });
}

/** 用户点击“我来接管”。 */
export async function browserUserTakeOver(sessionId?: string | null): Promise<BrowserState> {
  if (!isTauriRuntime()) {
    return loadBrowserStatus();
  }
  return invokeLogged<BrowserState>("browser_user_take_over", { sessionId: sessionId ?? null });
}

/** 面板关闭某个标签页。 */
export async function browserCloseTab(tabId: string): Promise<BrowserState> {
  if (!isTauriRuntime()) {
    return closeMockBrowserTab(tabId);
  }
  return invokeLogged<BrowserState>("browser_close_tab", { tabId });
}

/** 清除站点数据：关闭浏览器并删除隔离 profile。 */
export async function browserClearSiteData(): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }
  return invokeLogged<void>("browser_clear_site_data");
}

/** 面板开关实时画面（CDP screencast）。 */
export async function browserSetScreencast(enabled: boolean): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }
  return invokeLogged<void>("browser_set_screencast", { enabled });
}

/** 审批弹窗的裁决入口。 */
export async function decideBrowserOperation(decision: BrowserApprovalDecision): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }
  return invokeLogged<void>("decide_browser_operation_approval", { decision });
}

/** 订阅浏览器状态变化。 */
export async function subscribeBrowserState(
  onState: (state: BrowserState) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<BrowserState>(BROWSER_STATE_EVENT, (event) => onState(event.payload));
}

/** 订阅面板实时画面帧。 */
export async function subscribeBrowserFrames(
  onFrame: (frame: BrowserFrame) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<BrowserFrame>(BROWSER_FRAME_EVENT, (event) => onFrame(event.payload));
}

/** 订阅 Agent 活动提示。 */
export async function subscribeBrowserActivity(
  onActivity: (activity: BrowserActivity) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<BrowserActivity>(BROWSER_ACTIVITY_EVENT, (event) => onActivity(event.payload));
}

/** 订阅操作凭证事件。 */
export async function subscribeBrowserReceipts(
  onReceipt: (receipt: BrowserReceipt) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<BrowserReceipt>(BROWSER_RECEIPT_EVENT, (event) => onReceipt(event.payload));
}

/** 订阅进阶级别下的操作审批请求。 */
export async function subscribeBrowserApprovals(
  onRequest: (request: BrowserApprovalRequest) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<BrowserApprovalRequest>(BROWSER_APPROVAL_EVENT, (event) =>
    onRequest(event.payload),
  );
}

/** 订阅 Chromium 下载完整状态，包括连接、解压以及成功或失败结果。 */
export async function subscribeBrowserDownloadProgress(
  onProgress: (status: ChromiumInstallStatus) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) {
    return () => undefined;
  }
  return listen<ChromiumInstallStatus>(
    BROWSER_DOWNLOAD_PROGRESS_EVENT,
    (event) => onProgress(event.payload),
  );
}
