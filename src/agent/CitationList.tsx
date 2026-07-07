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
    <section className="citation-list" aria-label="回答引用来源">
      <div className="citation-list-header">
        <strong>证据</strong>
        <span>
          {citations.length} 条引用 · {sourceCount} 个资料库
        </span>
      </div>
      <div className="citation-items">
        {citations.map((citation) => (
          <article className="citation" key={`${citation.noteId}-${citation.path}`}>
            <OverflowTooltipText as="strong" text={citation.title} logArea="agent_citation_title" />
            <OverflowTooltipText text={`${citation.knowledgeBaseName} · ${citation.path}`} logArea="agent_citation_path" />
            <p>{citation.snippet}</p>
          </article>
        ))}
      </div>
    </section>
  );
}
