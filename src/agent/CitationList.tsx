import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { MarkdownLink } from "../shared/MarkdownLink";
import type { Citation } from "../shared/types";

function isWebCitation(citation: Citation): boolean {
  return citation.kind === "web" && Boolean(citation.url);
}

/** 文档引用的可读定位标签：PDF 用页码，DOCX 用结构块/标题路径。 */
export function formatCitationTarget(citation: Citation): string {
  if (citation.location) {
    return citation.location;
  }
  if (citation.sourceType === "document") {
    if (citation.page) {
      return `第 ${citation.page} 页`;
    }
    if (citation.blockIndex) {
      return `结构块 ${citation.blockIndex}`;
    }
  }
  return "";
}

/** 引用来源列表。笔记/文档引用可点击跳转，网页引用打开外链。 */
export function CitationList({
  citations,
  onOpenCitation,
}: {
  citations?: Citation[];
  /** 点击资料库引用时打开对应文件并定位到页码/结构块。 */
  onOpenCitation?: (citation: Citation) => void;
}) {
  if (!citations?.length) {
    return null;
  }

  const webCitations = citations.filter(isWebCitation);
  const noteCitations = citations.filter((citation) => !isWebCitation(citation));
  const sourceCount = new Set(noteCitations.map((citation) => citation.knowledgeBaseName)).size;
  const summary = noteCitations.length > 0 && webCitations.length > 0
    ? `来源 · ${noteCitations.length} 条资料库 · ${webCitations.length} 条网页`
    : webCitations.length > 0
      ? `来源 · ${webCitations.length} 条网页`
      : `来源 · ${noteCitations.length} 条 · ${sourceCount} 个资料库`;

  return (
    <section className="mt-3 flex flex-wrap items-center gap-1.5" aria-label="回答引用来源">
      <span className="text-[11px] text-ink-soft">{summary}</span>
      {noteCitations.map((citation) => {
        const target = formatCitationTarget(citation);
        const title = `${citation.knowledgeBaseName} · ${citation.path}${target ? ` · ${target}` : ""}\n${citation.snippet}`;
        const content = (
          <>
            {/* 标题和位置都限制在引用条内，避免长路径撑出整个对话栏的横向滚动。 */}
            <OverflowTooltipText className="block max-w-full truncate" text={citation.title} logArea="agent_citation_title" />
            {target ? (
              <span className="block max-w-full truncate text-[11px] text-ink-soft" title={target}>
                {target}
              </span>
            ) : null}
          </>
        );

        return onOpenCitation ? (
          <button
            className="inline-flex min-w-0 max-w-[min(280px,100%)] cursor-pointer flex-col items-start rounded-full border border-border bg-surface px-2 py-1 text-left text-[11px] text-ink-muted hover:border-accent hover:text-ink"
            key={`note-${citation.noteId}-${citation.path}-${target}`}
            title={title}
            type="button"
            onClick={() => onOpenCitation(citation)}
          >
            {content}
          </button>
        ) : (
          <span
            className="inline-flex min-w-0 max-w-[min(280px,100%)] flex-col items-start rounded-full border border-border bg-surface px-2 py-1 text-[11px] text-ink-muted"
            key={`note-${citation.noteId}-${citation.path}`}
            title={title}
          >
            {content}
          </span>
        );
      })}
      {webCitations.map((citation) => (
        <MarkdownLink
          className="inline-flex min-w-0 max-w-[min(220px,100%)] items-center rounded-full border border-border bg-surface px-2 py-0.5 text-[11px] text-ink-muted"
          href={citation.url}
          key={`web-${citation.url}`}
          source="agent_message"
          title={citation.publishedAt ? `${citation.url} · ${citation.publishedAt}\n${citation.snippet}` : `${citation.url}\n${citation.snippet}`}
        >
          <OverflowTooltipText className="block min-w-0 truncate" text={citation.title} logArea="agent_citation_title" />
        </MarkdownLink>
      ))}
    </section>
  );
}