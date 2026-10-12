import { Check, Copy } from "lucide-react";
import { Children, isValidElement, useEffect, useRef, useState, type ReactNode } from "react";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button } from "./Button";
import { copyTextToClipboard } from "./clipboard";
import { cn } from "./cn";
import { codeBlockLanguageLabel, highlightCode, type CodeTokenKind } from "./highlightCode";
import { logWarn } from "./logger";
import { MarkdownLink, type MarkdownLinkSource } from "./MarkdownLink";
import { remarkRestoreProtectedTablePipes } from "./protectMarkdownTablePipes";

export { protectGfmTablePipesInInlineCode } from "./protectMarkdownTablePipes";

/** 预览和 Agent 消息共用：GFM 表格 + 还原行内代码里被保护的竖线。 */
export const markdownRemarkPlugins = [remarkGfm, remarkRestoreProtectedTablePipes];

/** Markdown 预览容器：编辑器阅读区。 */
export const markdownPreviewClassName =
  "min-h-0 min-w-0 overflow-auto bg-surface p-1 text-sm leading-[1.76] text-ink [scrollbar-gutter:stable] [&>:first-child]:mt-0 [&>:last-child]:mb-0";

/** Agent 消息里的 Markdown 容器。 */
export const markdownMessageClassName =
  "mt-2 text-[13px] leading-[1.62] text-ink [overflow-wrap:anywhere] [&>:first-child]:mt-0 [&>:last-child]:mb-0";

const previewHeadingClassName = "mt-[1.2em] mb-[0.45em] leading-[1.28] text-ink-strong";
const messageHeadingClassName = "mt-[0.95em] mb-[0.35em] leading-[1.28] text-ink-strong";

/** 语法色对应 app.css 里的浅色代码 token。plain 继承正文色。 */
const codeTokenClassName: Record<CodeTokenKind, string> = {
  plain: "",
  comment: "text-code-comment",
  keyword: "text-code-keyword",
  string: "text-code-string",
  number: "text-code-number",
  function: "text-code-function",
  type: "text-code-type",
  property: "text-code-property",
  constant: "text-code-number",
};

/** 围栏代码块：铺满所在栏，浅底、可横向滚动，并标出语言。 */
function MarkdownCodeBlock({ children, compact }: { children: ReactNode; compact: boolean }) {
  const { source, language } = readFencedCode(children);
  const label = language ? codeBlockLanguageLabel(language) : "代码";
  const tokens = highlightCode(source, language);
  const [copied, setCopied] = useState(false);
  const resetTimerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current != null) {
        window.clearTimeout(resetTimerRef.current);
      }
    };
  }, []);

  return (
    <div
      className={cn(
        "my-3 block w-full min-w-0 max-w-full overflow-hidden rounded-lg border border-border bg-surface-muted",
        compact && "my-2",
      )}
    >
      <div className="flex items-center justify-between gap-2 border-b border-border px-2.5 py-0.5">
        <span className="min-w-0 truncate font-mono text-[11px] tracking-[0.02em] text-ink-muted">{label}</span>
        <Button
          variant="icon"
          size="compact"
          className="text-ink-soft"
          title={copied ? "已复制" : "复制代码"}
          aria-label={copied ? "已复制" : "复制代码"}
          onClick={() => {
            void copyTextToClipboard(source)
              .then(() => {
                setCopied(true);
                if (resetTimerRef.current != null) {
                  window.clearTimeout(resetTimerRef.current);
                }
                resetTimerRef.current = window.setTimeout(() => setCopied(false), 1600);
              })
              .catch((error: unknown) => {
                logWarn("复制代码块失败", {
                  category: "frontend",
                  event: "copy_code_block",
                  error,
                  metadata: { contentLength: source.length },
                });
              });
          }}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </Button>
      </div>
      <pre className="m-0 max-w-full overflow-x-auto bg-transparent px-3 py-2.5 font-mono text-[12.5px] leading-[1.65] whitespace-pre text-ink [overflow-wrap:normal] [word-break:normal]">
        <code>
          {tokens.map((token, index) => (
            <span className={codeTokenClassName[token.kind] || undefined} key={`${token.kind}-${index}`}>
              {token.text}
            </span>
          ))}
        </code>
      </pre>
    </div>
  );
}

/** 从围栏子节点取出源码和语言名。react-markdown 会包一层带 language-* class 的 code。 */
function readFencedCode(children: ReactNode): { source: string; language: string } {
  const codeChild = Children.toArray(children).find((child) => isValidElement(child));
  if (isValidElement<{ className?: string; children?: ReactNode }>(codeChild)) {
    const match = /language-([\w#+.-]+)/.exec(codeChild.props.className ?? "");
    return {
      source: nodeToText(codeChild.props.children).replace(/\n$/, ""),
      language: match?.[1] ?? "",
    };
  }

  return { source: nodeToText(children).replace(/\n$/, ""), language: "" };
}

/** 把 React 子节点收成纯文本，供高亮器按原文切分。 */
function nodeToText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") {
    return String(node);
  }

  if (Array.isArray(node)) {
    return node.map((child) => nodeToText(child)).join("");
  }

  if (isValidElement<{ children?: ReactNode }>(node)) {
    return nodeToText(node.props.children);
  }

  return "";
}

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
          "rounded-[5px] bg-surface-muted font-mono text-[0.92em] [overflow-wrap:anywhere]",
          isPreview ? "px-[5px] py-0.5" : "px-1 py-px",
          className,
        )}
        {...props}
      />
    ),
    pre: ({ node: _node, children }) => <MarkdownCodeBlock compact={!isPreview}>{children}</MarkdownCodeBlock>,
    table: ({ node: _node, className, ...props }) => (
      <table
        className={cn(
          blockMarginClassName,
          "w-full max-w-full overflow-hidden rounded-xl border border-border border-collapse",
          className,
        )}
        {...props}
      />
    ),
    th: ({ node: _node, className, ...props }) => (
      <th
        className={cn(
          "border-b border-border bg-surface-muted text-left align-top text-[11px] font-medium tracking-[0.04em] text-ink-muted",
          isPreview ? "px-3 py-2" : "px-2.5 py-1.5",
          className,
        )}
        {...props}
      />
    ),
    td: ({ node: _node, className, ...props }) => (
      <td
        className={cn(
          "border-b border-border text-left align-top last:border-b-0",
          isPreview ? "px-3 py-2" : "px-2.5 py-1.5",
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
