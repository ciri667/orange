/**
 * 网页链接默认打开方式判定。
 *
 * 这里只放纯函数，方便在没有 Tauri / 浏览器的环境里用 node:test 覆盖
 * “什么时候该用内置浏览器、什么时候回退系统默认浏览器”。
 */

/** 判定所需的运行时事实；调用方负责从设置和 BrowserState 里取。 */
export interface ExternalLinkTargetInput {
  /** 是否为内置浏览器能承载的 http/https 网页；mailto/tel 等始终交给系统。 */
  isWebPage: boolean;
  /** 用户按住 Ctrl / Cmd 点击，明确要求系统默认浏览器。 */
  bypassEmbeddedBrowser: boolean;
  /** 是否运行在 Tauri 桌面壳中；网页开发态没有内置浏览器。 */
  isDesktopRuntime: boolean;
  /** 设置里是否启用了内嵌浏览器。 */
  browserEnabled: boolean;
  /** 设置里的“网页链接默认在内置浏览器打开”。 */
  openLinksInEmbedded: boolean;
  /** 固定版本 Chromium 是否已安装。 */
  chromiumInstalled: boolean;
}

/** 目标打开方式；`notice` 有值时调用方在回退系统浏览器后向用户说明原因。 */
export interface ExternalLinkTarget {
  target: "embedded" | "system";
  notice?: string;
}

/** 内核未安装时的提示文案；回退系统浏览器后仍需让用户知道为什么没走内置。 */
export const CHROMIUM_MISSING_NOTICE = "未安装浏览器内核，已用系统默认浏览器打开。";

/** 判定一次网页链接点击应该落在内置浏览器还是系统默认浏览器。 */
export function resolveExternalLinkTarget(input: ExternalLinkTargetInput): ExternalLinkTarget {
  const system = { target: "system" } as const;

  if (!input.isWebPage || !input.isDesktopRuntime || input.bypassEmbeddedBrowser) {
    return system;
  }

  if (!input.browserEnabled || !input.openLinksInEmbedded) {
    return system;
  }

  if (!input.chromiumInstalled) {
    // 功能开启但内核缺失：提示一次并回退，避免用户点击后没有任何反应。
    return { target: "system", notice: CHROMIUM_MISSING_NOTICE };
  }

  return { target: "embedded" };
}
