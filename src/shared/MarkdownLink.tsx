import type { AnchorHTMLAttributes, MouseEvent } from "react";
import { useExternalLinkHandler, type ExternalLinkHandler } from "./externalLink";
import { logInfo, logWarn } from "./logger";
import { openExternalUrl } from "./openExternalUrl";

/** Markdown 链接来源，用于日志区分编辑器预览和 Agent 消息。 */
export type MarkdownLinkSource = "editor_preview" | "agent_message";

/** 外部链接允许的协议集合；收敛协议可以避免 markdown 内容触发危险或非预期系统调用。 */
const ALLOWED_EXTERNAL_LINK_PROTOCOLS = new Set(["http:", "https:", "mailto:", "tel:"]);

/** Markdown 链接渲染属性，复用原生 a 标签属性并追加来源字段。 */
interface MarkdownLinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  source: MarkdownLinkSource;
}

/** 安全 Markdown 链接组件，优先交给内置浏览器，缺少入口时才用系统默认应用。 */
export function MarkdownLink({ href, source, children, onClick, ...props }: MarkdownLinkProps) {
  const linkSummary = summarizeMarkdownLink(href);
  const isExternalLink = Boolean(linkSummary?.isAllowedExternal);
  const routeExternalLink = useExternalLinkHandler();

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

    // Ctrl / Cmd 点击沿用浏览器习惯：用户明确要求交给系统默认浏览器，不进内置面板。
    const bypassEmbeddedBrowser = event.metaKey || event.ctrlKey;
    void openAllowedExternalLink(linkSummary, source, routeExternalLink, bypassEmbeddedBrowser);
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

/**
 * 打开已校验外部链接：先问工作区能否交给内置浏览器，未接手才用系统默认应用。
 * 只记录协议、域名等脱敏摘要，不把完整 URL 写进日志。
 */
async function openAllowedExternalLink(
  summary: MarkdownLinkSummary,
  source: MarkdownLinkSource,
  routeExternalLink: ExternalLinkHandler | null,
  bypassEmbeddedBrowser: boolean,
) {
  try {
    let handledByEmbeddedBrowser = false;

    if (summary.isWebPage && !bypassEmbeddedBrowser && routeExternalLink) {
      // 内置浏览器打不开时返回 false，由下面回退系统默认浏览器，保证点击总有结果。
      handledByEmbeddedBrowser = await routeExternalLink({
        href: summary.href,
        source,
        isWebPage: true,
      });
    }

    if (!handledByEmbeddedBrowser) {
      await openExternalUrl(summary.href);
    }

    logInfo(
      handledByEmbeddedBrowser
        ? "已在内置浏览器打开 Markdown 外部链接。"
        : "已通过系统默认应用打开 Markdown 外部链接。",
      {
        category: "frontend",
        event: "markdown_external_link_opened",
        status: "completed",
        metadata: {
          source,
          protocol: summary.protocol,
          host: summary.host,
          target: handledByEmbeddedBrowser ? "embedded" : "system",
        },
      },
    );
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
  /** 是否为内置浏览器能承载的普通网页（http/https）。 */
  isWebPage: boolean;
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
      isWebPage: false,
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
      isWebPage: parsedUrl.protocol === "http:" || parsedUrl.protocol === "https:",
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
      isWebPage: false,
      href: trimmedHref,
      protocol: "relative",
      host: "",
      reason: "relative_link_unsupported",
    };
  }
}
