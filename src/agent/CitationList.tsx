import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { Citation } from "../shared/types";

/** 引用来源列表，帮助用户追溯 Agent 回答依据和知识库边界。 */
export function CitationList({ citations }: { citations?: Citation[] }) {
  if (!citations?.length) {
    return null;
  }

  /** 引用来源按知识库去重，用于引用块标题的低噪音摘要。 */
  const sourceCount = new Set(citations.map((citation) => citation.knowledgeBaseName)).size;

  return (
    <section className="mt-3 flex flex-wrap items-center gap-1.5" aria-label="回答引用来源">
      <span className="text-[11px] text-ink-soft">
        来源 · {citations.length} 条 · {sourceCount} 个资料库
      </span>
      {citations.map((citation) => (
        <span
          className="inline-flex max-w-[220px] items-center rounded-full border border-border bg-surface px-2 py-0.5 text-[11px] text-ink-muted"
          key={`${citation.noteId}-${citation.path}`}
          title={`${citation.knowledgeBaseName} · ${citation.path}${citation.location ? ` · ${citation.location}` : ""}\n${citation.snippet}`}
        >
          <OverflowTooltipText text={citation.title} logArea="agent_citation_title" />
        </span>
      ))}
    </section>
  );
}
