import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauriRuntime } from "./api/runtime";

/** 判断地址是否是内置浏览器能承载的普通网页；mailto/tel 等仍交给系统默认应用。 */
export function isWebPageUrl(value?: string | null): boolean {
  const trimmed = value?.trim();

  if (!trimmed) {
    return false;
  }

  try {
    const protocol = new URL(trimmed).protocol;
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * 把链接交给系统默认应用（浏览器、邮件客户端等）。
 *
 * 桌面端走 Tauri opener，避免主 WebView 导航到外部页面；
 * 网页开发态没有 opener 权限，退化为新窗口打开并沿用浏览器拦截检测。
 */
export async function openExternalUrl(href: string): Promise<void> {
  if (!isTauriRuntime()) {
    const browserWindow = globalThis.open?.(href, "_blank", "noopener,noreferrer");

    if (!browserWindow) {
      throw new Error("浏览器拦截了外部链接。");
    }
    return;
  }

  await openUrl(href);
}
