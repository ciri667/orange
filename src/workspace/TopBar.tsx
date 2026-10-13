import { MessageSquarePlus, PanelRight, PanelTopClose, PanelTopOpen, Settings } from "lucide-react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";

/** 窄屏顶栏：侧栏隐藏时提供品牌、新对话、Agent 和设置入口。 */
export function TopBar({
  onOpenSettings,
  agentOpen,
  onToggleAgent,
  onCreateSession,
  height,
  collapsed = false,
  onToggleCollapsed,
}: {
  onOpenSettings: () => void;
  agentOpen?: boolean;
  onToggleAgent?: () => void;
  onCreateSession?: () => void;
  /** 展开态高度由布局 hook 给出，收起时压到横向图标栏厚度。 */
  height?: number;
  /** 顶栏是否已完全收起；收起后只留一个展开按钮。 */
  collapsed?: boolean;
  onToggleCollapsed?: () => void;
}) {
  /** 顶栏在所有宽度下都保留，因此不再用 max-[760px] 限制显隐。 */
  const style = height === undefined ? undefined : { height };

  if (collapsed) {
    return (
      <header className="top-bar is-collapsed" style={style}>
        {/* 收起态横向图标栏：左侧品牌加一个展开按钮，避免留下一条空白横条。 */}
        <div className="region-rail region-rail-top">
          <ul className="region-rail-list">
            <li>
              <button
                type="button"
                className="region-rail-item is-active"
                title="展开顶栏"
                aria-label="展开顶栏"
                aria-expanded={false}
                onClick={onToggleCollapsed}
              >
                <PanelTopOpen size={16} aria-hidden />
              </button>
            </li>
          </ul>
        </div>
      </header>
    );
  }

  return (
    <header className="top-bar" style={style}>
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
        {onToggleCollapsed && (
          <Button variant="icon" title="收起顶栏" aria-label="收起顶栏" onClick={onToggleCollapsed}>
            <PanelTopClose size={16} />
          </Button>
        )}
      </div>
    </header>
  );
}
