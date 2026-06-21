import { History, Settings, ShieldCheck } from "lucide-react";
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
          <span>
            {knowledgeBaseCount} 个本地知识库 · 当前：{activeKnowledgeBase.name}
          </span>
        </div>
      </div>
      <div className="topbar-status">
        <span>
          <ShieldCheck size={15} />
          工具受控
        </span>
        <span>
          <History size={15} />
          {activeKnowledgeBase.updatedAt} 已索引
        </span>
        <button className="icon-button" type="button" title="打开设置" onClick={onOpenSettings}>
          <Settings size={18} />
        </button>
      </div>
    </header>
  );
}
