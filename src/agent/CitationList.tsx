import type { Citation } from "../shared/types";

/** 引用来源列表，帮助用户追溯 Agent 回答依据和知识库边界。 */
export function CitationList({ citations }: { citations?: Citation[] }) {
  if (!citations?.length) {
    return null;
  }

  return (
    <div className="citation-list">
      {citations.map((citation) => (
        <div className="citation" key={`${citation.noteId}-${citation.path}`}>
          <strong>{citation.title}</strong>
          <span>
            {citation.knowledgeBaseName} · {citation.path}
          </span>
          <p>{citation.snippet}</p>
        </div>
      ))}
    </div>
  );
}
