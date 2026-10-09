import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { MarkdownLink } from "../shared/MarkdownLink";
import type { Citation } from "../shared/types";

function isWebCitation(citation: Citation): boolean {
  return citation.kind === "web" && Boolean(citation.url);
}

/** 引用来源列表。笔记保持原来的资料库芯片，网页引用可以打开外链。 */
export function CitationList({ citations }: { citations?: Citation[] }) {
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
      {noteCitations.map((citation) => (
        <span
          className="inline-flex max-w-[220px] items-center rounded-full border border-border bg-surface px-2 py-0.5 text-[11px] text-ink-muted"
          key={`note-${citation.noteId}-${citation.path}`}
          title={`${citation.knowledgeBaseName} · ${citation.path}${citation.location ? ` · ${citation.location}` : ""}\n${citation.snippet}`}
        >
          <OverflowTooltipText text={citation.title} logArea="agent_citation_title" />
        </span>
      ))}
      {webCitations.map((citation) => (
        <MarkdownLink
          className="inline-flex max-w-[220px] items-center rounded-full border border-border bg-surface px-2 py-0.5 text-[11px] text-ink-muted"
          href={citation.url}
          key={`web-${citation.url}`}
          source="agent_message"
          title={citation.publishedAt ? `${citation.url} · ${citation.publishedAt}\n${citation.snippet}` : `${citation.url}\n${citation.snippet}`}
        >
          <OverflowTooltipText text={citation.title} logArea="agent_citation_title" />
        </MarkdownLink>
      ))}
    </section>
  );
}
