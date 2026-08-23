import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { Citation } from "../shared/types";

/** 引用来源列表，帮助用户追溯 Agent 回答依据和知识库边界。 */
export function CitationList({ citations }: { citations?: Citation[] }) {
  if (!citations?.length) {
    return null;
  }

  /** 引用来源按知识库去重，用于证据块标题的低噪音摘要。 */
  const sourceCount = new Set(citations.map((citation) => citation.knowledgeBaseName)).size;

  return (
    <section className="mt-2.5 grid gap-[7px]" aria-label="回答引用来源">
      <div className="flex items-center justify-between gap-2 text-[11px] text-ink-muted">
        <strong className="text-xs text-agent-strong">证据</strong>
        <span>
          {citations.length} 条引用 · {sourceCount} 个资料库
        </span>
      </div>
      <div className="grid gap-1.5">
        {citations.map((citation) => (
          <article className="rounded-r-control border-l-[3px] border-agent bg-primary-wash px-[9px] py-[7px]" key={`${citation.noteId}-${citation.path}`}>
            <OverflowTooltipText as="strong" className="block" text={citation.title} logArea="agent_citation_title" />
            <OverflowTooltipText className="mt-[3px] block text-xs text-ink-muted" text={`${citation.knowledgeBaseName} · ${citation.path}${citation.location ? ` · ${citation.location}` : ""}`} logArea="agent_citation_path" />
            <p className="mt-[3px] mb-0 text-xs">{citation.snippet}</p>
          </article>
        ))}
      </div>
    </section>
  );
}
