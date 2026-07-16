import { Database, Settings } from "lucide-react";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { KnowledgeBase } from "../shared/types";

/** 顶部应用栏，承载产品状态、激活知识库同步状态、Agent 浮窗开关和设置入口。 */
export function TopBar({
  activeKnowledgeBase,
  knowledgeBaseCount,
  onOpenSettings,
  agentOpen,
  onToggleAgent,
}: {
  activeKnowledgeBase: KnowledgeBase;
  knowledgeBaseCount: number;
  onOpenSettings: () => void;
  /** Agent 浮窗是否打开；仅在提供 onToggleAgent 时参与渲染。 */
  agentOpen?: boolean;
  /** 切换 Agent 协作浮窗显隐；未提供时不渲染顶部 Agent 按钮。 */
  onToggleAgent?: () => void;
}) {
  return (
    <header className="topbar">
      <div className="app-identity">
        <div className="brand-mark small">
          <img className="brand-logo" src="/orange-logo.svg" alt="" />
        </div>
        <div>
          <strong>橘记</strong>
          <span>个人 Agent 笔记</span>
        </div>
      </div>
      <div className="topbar-context" aria-label="当前知识库">
        <Database size={15} />
        <div>
          <span>当前资料库</span>
          <OverflowTooltipText as="strong" text={activeKnowledgeBase.name} logArea="topbar_active_knowledge_base" />
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
        {onToggleAgent && (
          <button
            type="button"
            className={`topbar-agent-toggle ${agentOpen ? "is-open" : ""}`}
            title={agentOpen ? "收起 Agent 协作区" : "打开 Agent 协作区"}
            aria-expanded={Boolean(agentOpen)}
            onClick={onToggleAgent}
          >
            Agent
          </button>
        )}
        <button className="icon-button" type="button" title="打开设置" onClick={onOpenSettings}>
          <Settings size={18} />
        </button>
      </div>
    </header>
  );
}
