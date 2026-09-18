import { MessageSquarePlus, PanelRight, Settings } from "lucide-react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";

/** 窄屏顶栏：侧栏隐藏时提供品牌、新对话、Agent 和设置入口。 */
export function TopBar({
  onOpenSettings,
  agentOpen,
  onToggleAgent,
  onCreateSession,
}: {
  onOpenSettings: () => void;
  agentOpen?: boolean;
  onToggleAgent?: () => void;
  onCreateSession?: () => void;
}) {
  return (
    <header className="hidden h-11 shrink-0 items-center justify-between border-b border-border bg-surface px-3 max-[760px]:flex">
      <div className="flex min-w-0 items-center gap-2">
        <div className="grid size-7 place-items-center overflow-hidden rounded-md">
          <img className="block size-full object-contain" src="/orange-logo.svg" alt="" />
        </div>
        <strong className="text-sm font-semibold text-ink-strong">橘记</strong>
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        {onCreateSession && (
          <Button variant="icon" title="新对话" onClick={onCreateSession}>
            <MessageSquarePlus size={16} />
          </Button>
        )}
        {onToggleAgent && (
          <Button
            variant="icon"
            title={agentOpen ? "收起 Agent" : "打开 Agent"}
            aria-expanded={Boolean(agentOpen)}
            className={cn(agentOpen && "bg-surface-muted text-ink")}
            onClick={onToggleAgent}
          >
            <PanelRight size={16} />
          </Button>
        )}
        <Button variant="icon" title="打开设置" onClick={onOpenSettings}>
          <Settings size={16} />
        </Button>
      </div>
    </header>
  );
}
