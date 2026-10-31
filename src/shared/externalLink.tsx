import { createContext, useContext, type ReactNode } from "react";
import type { MarkdownLinkSource } from "./MarkdownLink";

/** 一次外部链接打开请求；source 只用于日志区分来源，不参与策略判定。 */
export interface ExternalLinkRequest {
  href: string;
  source: MarkdownLinkSource;
  /** 是否为内置浏览器能承载的 http/https 网页；mailto/tel 等始终交给系统。 */
  isWebPage: boolean;
}

/** 返回 true 表示已由工作区内置浏览器接手；false 让调用方回退到系统默认应用。 */
export type ExternalLinkHandler = (request: ExternalLinkRequest) => boolean | Promise<boolean>;

/**
 * 链接打开策略的注入点。
 *
 * `shared` 只定义接口，真正的“默认用内置浏览器打开”由工作区注入；
 * 没有 Provider 的渲染环境（独立预览、开发页）保持系统默认应用行为。
 */
const ExternalLinkContext = createContext<ExternalLinkHandler | null>(null);

/** 在工作区根节点注入链接策略，让 Markdown 渲染链无需逐层透传回调。 */
export function ExternalLinkProvider({
  handler,
  children,
}: {
  handler: ExternalLinkHandler;
  children: ReactNode;
}) {
  return <ExternalLinkContext.Provider value={handler}>{children}</ExternalLinkContext.Provider>;
}

/** 读取工作区注入的链接策略；返回 null 表示当前环境没有内置浏览器入口。 */
export function useExternalLinkHandler(): ExternalLinkHandler | null {
  return useContext(ExternalLinkContext);
}
