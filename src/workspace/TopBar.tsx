import { Database, Settings } from "lucide-react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { KnowledgeBase } from "../shared/types";

/** 顶部应用栏，承载产品状态、激活知识库同步状态、Agent 停靠栏开关和设置入口。 */
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
  /** 右侧 Agent 停靠栏是否打开；仅在提供 onToggleAgent 时参与渲染。 */
  agentOpen?: boolean;
  /** 切换右侧 Agent 停靠栏显隐；未提供时不渲染顶部 Agent 按钮。 */
  onToggleAgent?: () => void;
}) {
  return (
    <header className="flex items-center justify-between border-b border-border-translucent bg-[rgba(var(--surface-warm-rgb),0.92)] px-4 backdrop-blur-[18px] max-[760px]:gap-2">
      <div className="flex w-[240px] shrink-0 items-center gap-[11px] max-[760px]:min-w-0 max-[760px]:flex-1">
        <div className="grid size-8 place-items-center overflow-hidden rounded-control bg-accent-soft text-accent-strong">
          <img className="block size-full object-contain" src="/orange-logo.svg" alt="" />
        </div>
        <div className="min-w-0">
          <strong className="block text-ink-strong">橘记</strong>
          <span className="block truncate text-xs text-ink-muted">个人 Agent 笔记</span>
        </div>
      </div>
      <div
        className="inline-flex min-w-0 max-w-[min(46vw,520px)] items-center gap-2.5 rounded-full border border-border-translucent bg-surface-translucent px-3 py-[5px] text-ink-muted max-[1180px]:max-w-[38vw] max-[760px]:hidden"
        aria-label="当前知识库"
      >
        <Database size={15} className="shrink-0 text-accent" />
        <div className="flex min-w-0 items-baseline gap-2">
          <span className="whitespace-nowrap text-xs">当前资料库</span>
          <OverflowTooltipText as="strong" className="min-w-0 truncate text-[13px] text-ink-strong" text={activeKnowledgeBase.name} logArea="topbar_active_knowledge_base" />
        </div>
        <em className="whitespace-nowrap text-xs not-italic text-ink-soft max-[1180px]:hidden">
          {knowledgeBaseCount} 个库 · {activeKnowledgeBase.updatedAt} 已索引
        </em>
      </div>
      <div className="flex shrink-0 items-center gap-2.5">
        <span className="inline-flex items-center gap-[7px] whitespace-nowrap text-xs text-ink-muted max-[980px]:hidden">
          <i className="size-[7px] rounded-full bg-success shadow-[0_0_0_4px_rgba(var(--success-rgb),0.12)]" aria-hidden="true" />
          写入需确认
        </span>
        {onToggleAgent && (
          <button
            type="button"
            className={cn(
              "rounded-full border border-[rgba(59,92,204,0.22)] bg-[rgba(232,236,255,0.72)] px-3 py-1.5 text-xs font-bold text-agent-strong",
              "hover:border-[rgba(59,92,204,0.4)]",
              agentOpen && "border-[rgba(59,92,204,0.4)] bg-agent-soft",
            )}
            title={agentOpen ? "收起 Agent 协作区" : "打开 Agent 协作区"}
            aria-expanded={Boolean(agentOpen)}
            onClick={onToggleAgent}
          >
            Agent
          </button>
        )}
        <Button variant="icon" title="打开设置" onClick={onOpenSettings}>
          <Settings size={18} />
        </Button>
      </div>
    </header>
  );
}
