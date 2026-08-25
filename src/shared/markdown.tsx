import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "./cn";
import { MarkdownLink, type MarkdownLinkSource } from "./MarkdownLink";
import { remarkRestoreProtectedTablePipes } from "./protectMarkdownTablePipes";

export { protectGfmTablePipesInInlineCode } from "./protectMarkdownTablePipes";

/** 预览和 Agent 消息共用：GFM 表格 + 还原行内代码里被保护的竖线。 */
export const markdownRemarkPlugins = [remarkGfm, remarkRestoreProtectedTablePipes];

/** Markdown 预览容器：编辑器阅读区。 */
export const markdownPreviewClassName =
  "min-h-0 min-w-0 overflow-auto rounded-panel border border-[rgba(230,224,214,0.92)] bg-surface p-[22px] text-sm leading-[1.76] text-ink [scrollbar-gutter:stable] [&>:first-child]:mt-0 [&>:last-child]:mb-0";

/** Agent 消息里的 Markdown 容器。 */
export const markdownMessageClassName =
  "mt-2 text-[13px] leading-[1.62] text-ink [overflow-wrap:anywhere] [&>:first-child]:mt-0 [&>:last-child]:mb-0";

const previewHeadingClassName = "mt-[1.2em] mb-[0.45em] leading-[1.28] text-ink-strong";
const messageHeadingClassName = "mt-[0.95em] mb-[0.35em] leading-[1.28] text-ink-strong";

/**
 * 给 ReactMarkdown 用的标签样式。
 * 预览区字号更大、段距更疏；消息气泡更紧凑。
 */
export function createMarkdownComponents(
  source: MarkdownLinkSource,
  variant: "preview" | "message",
): Components {
  const isPreview = variant === "preview";
  const headingClassName = isPreview ? previewHeadingClassName : messageHeadingClassName;
  const blockMarginClassName = isPreview ? "mb-4" : "mt-2";

  return {
    h1: ({ node: _node, className, ...props }) => (
      <h1
        className={cn(headingClassName, isPreview ? "border-b border-border pb-[0.25em] text-2xl" : "text-lg", className)}
        {...props}
      />
    ),
    h2: ({ node: _node, className, ...props }) => (
      <h2 className={cn(headingClassName, isPreview ? "text-[22px]" : "text-base", className)} {...props} />
    ),
    h3: ({ node: _node, className, ...props }) => (
      <h3 className={cn(headingClassName, isPreview ? "text-lg" : "text-sm", className)} {...props} />
    ),
    h4: ({ node: _node, className, ...props }) => (
      <h4 className={cn(headingClassName, isPreview ? "text-lg" : "text-sm", className)} {...props} />
    ),
    h5: ({ node: _node, className, ...props }) => (
      <h5 className={cn(headingClassName, isPreview ? "text-lg" : "text-sm", className)} {...props} />
    ),
    h6: ({ node: _node, className, ...props }) => (
      <h6 className={cn(headingClassName, isPreview ? "text-lg" : "text-sm", className)} {...props} />
    ),
    p: ({ node: _node, className, ...props }) => (
      <p className={cn(blockMarginClassName, className)} {...props} />
    ),
    ul: ({ node: _node, className, ...props }) => (
      <ul className={cn(blockMarginClassName, isPreview ? "pl-6" : "pl-[1.35em]", className)} {...props} />
    ),
    ol: ({ node: _node, className, ...props }) => (
      <ol className={cn(blockMarginClassName, isPreview ? "pl-6" : "pl-[1.35em]", className)} {...props} />
    ),
    li: ({ node: _node, className, ...props }) => (
      <li className={cn(isPreview ? "my-[0.2em]" : "my-[0.18em]", className)} {...props} />
    ),
    blockquote: ({ node: _node, className, ...props }) => (
      <blockquote
        className={cn(
          blockMarginClassName,
          "border-l-[3px] border-border-strong py-0.5 text-ink-muted",
          isPreview ? "pl-3" : "pl-2.5",
          className,
        )}
        {...props}
      />
    ),
    a: ({ node: _node, ...props }) => (
      <MarkdownLink
        {...props}
        source={source}
        className={cn("text-agent-strong underline underline-offset-2", props.className)}
      />
    ),
    img: ({ node: _node, className, ...props }) => (
      <img className={cn("mb-4 block h-auto max-w-full rounded-md", className)} {...props} />
    ),
    code: ({ node: _node, className, ...props }) => (
      <code
        className={cn(
          "rounded-[5px] bg-[#f1eee8] font-mono text-[0.92em]",
          isPreview ? "px-[5px] py-0.5" : "px-1 py-px",
          className,
        )}
        {...props}
      />
    ),
    pre: ({ node: _node, className, ...props }) => (
      <pre
        className={cn(
          blockMarginClassName,
          "max-w-full overflow-auto rounded-[7px] bg-[#171717] text-[#f7f6f2] [&_code]:bg-transparent [&_code]:p-0 [&_code]:text-inherit",
          isPreview ? "p-[13px]" : "p-2.5",
          className,
        )}
        {...props}
      />
    ),
    table: ({ node: _node, className, ...props }) => (
      <table
        className={cn(
          blockMarginClassName,
          "block w-full max-w-full overflow-x-auto rounded-control border-collapse",
          className,
        )}
        {...props}
      />
    ),
    th: ({ node: _node, className, ...props }) => (
      <th
        className={cn(
          "border border-border bg-warm-panel text-left align-top font-bold text-ink-strong",
          isPreview ? "px-[9px] py-[7px]" : "px-2 py-1.5",
          className,
        )}
        {...props}
      />
    ),
    td: ({ node: _node, className, ...props }) => (
      <td
        className={cn(
          "border border-border text-left align-top",
          isPreview ? "px-[9px] py-[7px]" : "px-2 py-1.5",
          className,
        )}
        {...props}
      />
    ),
    input: ({ node: _node, className, ...props }) =>
      props.type === "checkbox" ? (
        <input
          {...props}
          className={cn(
            "mr-[7px] size-[14px] align-[-2px] rounded border border-border-strong bg-white",
            "checked:border-accent checked:bg-[linear-gradient(135deg,transparent_45%,#ffffff_45%_55%,transparent_55%),var(--accent)]",
            className,
          )}
        />
      ) : (
        <input {...props} className={className} />
      ),
  };
}
