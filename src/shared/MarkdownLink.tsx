import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "@tauri-apps/api/core";
import type { AnchorHTMLAttributes, MouseEvent } from "react";
import { logInfo, logWarn } from "./logger";

/** Markdown 链接来源，用于日志区分编辑器预览和 Agent 消息。 */
export type MarkdownLinkSource = "editor_preview" | "agent_message";

/** 外部链接允许的协议集合；收敛协议可以避免 markdown 内容触发危险或非预期系统调用。 */
const ALLOWED_EXTERNAL_LINK_PROTOCOLS = new Set(["http:", "https:", "mailto:", "tel:"]);

/** Markdown 链接渲染属性，复用原生 a 标签属性并追加来源字段。 */
interface MarkdownLinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  source: MarkdownLinkSource;
}

/** 安全 Markdown 链接组件，拦截外部链接并交给系统默认应用打开。 */
export function MarkdownLink({ href, source, children, onClick, ...props }: MarkdownLinkProps) {
  const linkSummary = summarizeMarkdownLink(href);
  const isExternalLink = Boolean(linkSummary?.isAllowedExternal);

  /** 点击链接时先执行上游回调，再按协议决定是否拦截默认 WebView 导航。 */
  const handleClick = (event: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(event);

    if (event.defaultPrevented || !href) {
      return;
    }

    if (linkSummary?.isFragment) {
      // 页面内锚点继续交给浏览器默认滚动处理，不需要系统浏览器参与。
      return;
    }

    event.preventDefault();

    if (!linkSummary?.isAllowedExternal) {
      logWarn("已拦截不支持的 Markdown 链接跳转。", {
        category: "security",
        event: "markdown_link_blocked",
        status: "blocked",
        metadata: {
          source,
          reason: linkSummary?.reason ?? "empty_href",
          protocol: linkSummary?.protocol ?? "unknown",
        },
      });
      return;
    }

    void openAllowedExternalLink(linkSummary.href, source, linkSummary);
  };

  return (
    <a
      {...props}
      href={href}
      onClick={handleClick}
      rel={isExternalLink ? "noreferrer" : props.rel}
      target={isExternalLink ? "_blank" : props.target}
    >
      {children}
    </a>
  );
}

/** 打开已校验外部链接，并只记录协议、域名等脱敏摘要。 */
async function openAllowedExternalLink(href: string, source: MarkdownLinkSource, summary: MarkdownLinkSummary) {
  try {
    if (!isTauri()) {
      const browserWindow = globalThis.open?.(href, "_blank", "noopener,noreferrer");

      if (!browserWindow) {
        throw new Error("Browser blocked opening the external link.");
      }

      logInfo("已在浏览器开发态打开 Markdown 外部链接。", {
        category: "frontend",
        event: "markdown_external_link_opened",
        status: "completed",
        metadata: {
          source,
          protocol: summary.protocol,
          host: summary.host,
          runtime: "browser",
        },
      });
      return;
    }

    // Tauri opener 负责调用系统默认浏览器或默认应用，避免主 WebView 导航到外部页面。
    await openUrl(href);
    logInfo("已通过系统默认应用打开 Markdown 外部链接。", {
      category: "frontend",
      event: "markdown_external_link_opened",
      status: "completed",
      metadata: {
        source,
        protocol: summary.protocol,
        host: summary.host,
      },
    });
  } catch (error) {
    logWarn("Markdown 外部链接打开失败。", {
      category: "frontend",
      event: "markdown_external_link_open_failed",
      status: "failed",
      error,
      metadata: {
        source,
        protocol: summary.protocol,
        host: summary.host,
      },
    });
  }
}

/** Markdown 链接摘要，只保留可观测但不泄露 query/hash 的字段。 */
interface MarkdownLinkSummary {
  isAllowedExternal: boolean;
  isFragment: boolean;
  href: string;
  protocol: string;
  host: string;
  reason?: string;
}

/** 解析链接 href；不把完整 URL 写入日志，避免 query/hash 中的 token 泄露。 */
function summarizeMarkdownLink(href?: string): MarkdownLinkSummary | null {
  const trimmedHref = href?.trim();

  if (!trimmedHref) {
    return null;
  }

  if (trimmedHref.startsWith("#")) {
    return {
      isAllowedExternal: false,
      isFragment: true,
      href: trimmedHref,
      protocol: "fragment",
      host: "",
    };
  }

  try {
    // 协议相对 URL 统一按 https 打开，避免继承 tauri:// 或 http://localhost 等 WebView 基地址。
    const normalizedHref = trimmedHref.startsWith("//") ? `https:${trimmedHref}` : trimmedHref;
    const parsedUrl = new URL(normalizedHref);
    const isAllowedExternal = ALLOWED_EXTERNAL_LINK_PROTOCOLS.has(parsedUrl.protocol);

    return {
      isAllowedExternal,
      isFragment: false,
      href: normalizedHref,
      protocol: parsedUrl.protocol.replace(/:$/, ""),
      host: parsedUrl.hostname || parsedUrl.protocol.replace(/:$/, ""),
      reason: isAllowedExternal ? undefined : "unsupported_protocol",
    };
  } catch {
    // todo: 后续可把相对 Markdown 文件链接解析为工作区内跳转，目前先拦截以避免 WebView 导航离开 app。
    return {
      isAllowedExternal: false,
      isFragment: false,
      href: trimmedHref,
      protocol: "relative",
      host: "",
      reason: "relative_link_unsupported",
    };
  }
}
