import { Database, Settings } from "lucide-react";
import type { KnowledgeBase } from "../shared/types";

/** 顶部应用栏，承载产品状态、激活知识库同步状态和设置入口。 */
export function TopBar({
  activeKnowledgeBase,
  knowledgeBaseCount,
  onOpenSettings,
}: {
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBaseCount: number;
  onOpenSettings: () => void;
}) {
  return (
    <header className="topbar">
      <div className="app-identity">
        <div className="brand-mark small">
          <img className="brand-logo" src="/cici-note-logo.svg" alt="" />
        </div>
        <div>
          <strong>Cici Note</strong>
          <span>个人 Agent 笔记</span>
        </div>
      </div>
      <div className="topbar-context" aria-label="当前知识库">
        <Database size={15} />
        <div>
          <span>当前资料库</span>
          <strong>{activeKnowledgeBase.name}</strong>
        </div>
        <em>
          {knowledgeBaseCount} 个库 · {activeKnowledgeBase.updatedAt} 已索引
        </em>
      </div>
      <div className="topbar-status">
        <span className="topbar-safety">
          <i aria-hidden="true" />
          写入需确认
        </span>
        <button className="icon-button" type="button" title="打开设置" onClick={onOpenSettings}>
          <Settings size={18} />
        </button>
      </div>
    </header>
  );
}
