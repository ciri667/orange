import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Globe, Minus, PanelLeftClose, Square, X } from "lucide-react";
import { Button } from "../shared/Button";
import { MenuItem, MenuPanel } from "../shared/Menu";
import { cn } from "../shared/cn";
import { useDismissable } from "../shared/useDismissable";
import { isTauriRuntime } from "../shared/api/runtime";
import { logError } from "../shared/logger";
import type { CompactWorkspacePane } from "./useCompactWorkspaceView";
import { OrangeMark } from "../shared/OrangeMark";

/** 顶栏一个下拉菜单的声明。 */
export interface TopBarMenuItem {
  id: string;
  label: string;
  /** 右侧展示的快捷键提示；没有真实快捷键时不要填，避免提示失效。 */
  shortcut?: string;
  disabled?: boolean;
}

/** 顶栏菜单 id；顺序即渲染顺序。 */
export type TopBarMenuId = "file" | "edit" | "view" | "help";

/** 顶栏菜单的静态声明：只描述有哪些菜单和条目，不含动作。 */
export const TOP_BAR_MENUS: { id: TopBarMenuId; label: string; items: TopBarMenuItem[] }[] = [
  {
    id: "file",
    label: "文件",
    items: [
      { id: "new-session", label: "新建会话", disabled: true },
      { id: "new-note", label: "新建笔记", disabled: true },
      { id: "new-document", label: "新建文档", disabled: true },
      { id: "save", label: "保存当前文件", disabled: true },
      { id: "export", label: "导出当前文件", disabled: true },
    ],
  },
  {
    id: "edit",
    label: "编辑",
    items: [
      { id: "undo", label: "撤销", disabled: true },
      { id: "redo", label: "重做", disabled: true },
      { id: "cut", label: "剪切", disabled: true },
      { id: "copy", label: "复制", disabled: true },
      { id: "paste", label: "粘贴", disabled: true },
      { id: "select-all", label: "全选", disabled: true },
    ],
  },
  {
    id: "view",
    label: "视图",
    items: [
      { id: "toggle-sidebar", label: "收展知识库栏", disabled: true },
      { id: "toggle-agent", label: "收展 Agent 协作区", disabled: true },
      { id: "toggle-schedules", label: "定时任务", disabled: true },
      { id: "open-settings", label: "设置", disabled: true },
    ],
  },
  {
    id: "help",
    label: "帮助",
    items: [
      { id: "open-log-folder", label: "打开日志目录", disabled: true },
      { id: "about", label: "关于橘记", disabled: true },
    ],
  },
];

export interface WorkspaceTopBarProps {
  /** 左栏是否展开，决定收展按钮的图标和提示。 */
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  browserEnabled?: boolean;
  browserOpen?: boolean;
  onToggleBrowser?: () => void;
  /** 窄窗口侧栏用可关闭的导航层，笔记和对话以标签切换。 */
  compact?: boolean;
  compactPane?: CompactWorkspacePane;
  onCompactPaneChange?: (pane: CompactWorkspacePane) => void;
}

/**
 * 单个顶栏菜单。
 *
 * 点击展开而不是悬浮展开：悬浮容易在横向扫过菜单栏时误开，
 * 触屏设备上也没有悬浮，两者行为会不一致。
 */
function TopBarMenu({ menu }: { menu: (typeof TOP_BAR_MENUS)[number] }) {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  const containerRef = useDismissable<HTMLDivElement>(open, close);

  return (
    <div ref={containerRef} className="relative inline-flex" data-no-drag>
      <button
        type="button"
        className={cn(
          "inline-flex min-h-7 items-center rounded-small border-0 bg-transparent px-2.5 text-ui text-ink-muted",
          "hover:bg-surface-hover hover:text-ink",
          open && "bg-surface-muted text-ink",
        )}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        {menu.label}
      </button>
      {open && (
        <MenuPanel placement="bottom-end" className="left-0 right-auto min-w-[188px]">
          {menu.items.map((item) => (
            <MenuItem key={item.id} disabled={item.disabled}>
              <span className="flex-1">{item.label}</span>
              {item.shortcut && (
                <kbd className="shrink-0 font-sans text-[11px] text-ink-soft">{item.shortcut}</kbd>
              )}
            </MenuItem>
          ))}
        </MenuPanel>
      )}
    </div>
  );
}

/**
 * 桌面端平台标识。
 *
 * 初始为 null 可以避免浏览器环境误显示窗口控制；Tauri 运行时挂载后再按平台切换。
 */
type DesktopPlatform = "windows" | "macos" | "other";

/** 标题栏交互控件不参与拖拽和双击最大化；Element 同时覆盖图标的 SVG 节点。 */
function isTitleBarControl(target: EventTarget | null) {
  return target instanceof Element && Boolean(
    target.closest("[data-no-drag], button, a, input, textarea, select, [role='button'], [role='menuitem']"),
  );
}

/** 捕获桌面窗口 IPC 的拒绝，保留诊断信息，避免冒泡为未处理的异步错误。 */
function runWindowCommand(command: string, action: () => Promise<void>) {
  void action().catch((error: unknown) => {
    logError("窗口操作失败。", {
      event: "window_command",
      command,
      status: "failed",
      error,
    });
  });
}

/**
 * 工作台顶栏，同时也是桌面端集成标题栏。
 *
 * Windows 关闭系统装饰后，这里负责拖拽和窗口控制；macOS 用 Overlay 把系统红绿灯
 * 叠进同一行，因此不需要自绘窗口控制。
 */
export function WorkspaceTopBar({ sidebarOpen, onToggleSidebar, browserEnabled, browserOpen, onToggleBrowser, compact, compactPane, onCompactPaneChange }: WorkspaceTopBarProps) {
  const [platform, setPlatform] = useState<DesktopPlatform | null>(null);

  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }

    // WebView UA 在 Windows/macOS 上足够稳定；这里只区分标题栏形态，不做能力判断。
    const userAgent = navigator.userAgent.toLowerCase();
    setPlatform(userAgent.includes("windows") ? "windows" : userAgent.includes("mac os") ? "macos" : "other");
  }, []);

  /** 标题栏空白区域拖拽；控件和菜单通过 data-no-drag 排除。 */
  const handleDragStart = (event: React.PointerEvent<HTMLElement>) => {
    if (!isTauriRuntime() || event.button !== 0 || isTitleBarControl(event.target)) {
      return;
    }
    runWindowCommand("window.start_dragging", () => getCurrentWindow().startDragging());
  };

  return (
    <header
      className={cn(
        "workspace-topbar flex shrink-0 select-none items-center gap-2",
        platform === "macos" && "pl-[76px]",
      )}
      aria-label="工作台标题栏"
      data-platform={platform ?? undefined}
      onPointerDown={handleDragStart}
      onDoubleClick={(event) => {
        if (isTauriRuntime() && !isTitleBarControl(event.target)) {
          runWindowCommand("window.toggle_maximize", () => getCurrentWindow().toggleMaximize());
        }
      }}
    >
      <div className="flex shrink-0 items-center gap-2 pl-1" data-no-drag>
        <OrangeMark className="size-7" />
        <strong className="text-sm font-semibold tracking-tight text-ink-strong">橘记</strong>
      </div>

      <Button
        variant="icon"
        data-no-drag
        title={sidebarOpen ? "收起知识库栏" : "展开知识库栏"}
        aria-label={sidebarOpen ? "收起知识库栏" : "展开知识库栏"}
        aria-pressed={sidebarOpen}
        onClick={onToggleSidebar}
      >
        <PanelLeftClose size={16} className={compact && !sidebarOpen ? "rotate-180" : undefined} />
      </Button>

      <nav className="flex min-w-0 items-center gap-0.5" aria-label="主菜单">
        {TOP_BAR_MENUS.map((menu) => (
          <TopBarMenu key={menu.id} menu={menu} />
        ))}
      </nav>

      <div className="compact-workspace-tabs" aria-label="工作区切换" data-no-drag>
        <Button className={cn("px-3 text-sm", compactPane === "editor" && "border-border-strong font-medium")} aria-pressed={compactPane === "editor"} onClick={() => onCompactPaneChange?.("editor")}>笔记</Button>
        <Button className={cn("px-3 text-sm", compactPane === "agent" && "border-border-strong font-medium")} aria-pressed={compactPane === "agent"} onClick={() => onCompactPaneChange?.("agent")}>对话</Button>
      </div>

      {browserEnabled && <Button variant="ghost" size="compact" className="ml-auto" data-no-drag aria-label={browserOpen ? "收起浏览器面板" : "打开浏览器面板"} aria-pressed={browserOpen} onClick={onToggleBrowser}><Globe size={14} />浏览器</Button>}

      {platform === "windows" && (
        <div className={cn("flex h-full items-stretch", !browserEnabled && "ml-auto")} data-no-drag>
          <button
            type="button"
            className="grid w-11 place-items-center border-0 bg-transparent text-ink-muted hover:bg-surface-hover hover:text-ink"
            aria-label="最小化"
            onClick={() => runWindowCommand("window.minimize", () => getCurrentWindow().minimize())}
          >
            <Minus size={16} />
          </button>
          <button
            type="button"
            className="grid w-11 place-items-center border-0 bg-transparent text-ink-muted hover:bg-surface-hover hover:text-ink"
            aria-label="最大化 / 还原"
            onClick={() => runWindowCommand("window.toggle_maximize", () => getCurrentWindow().toggleMaximize())}
          >
            <Square size={14} />
          </button>
          <button
            type="button"
            className="grid w-11 place-items-center border-0 bg-transparent text-ink-muted hover:bg-danger hover:text-white"
            aria-label="关闭"
            onClick={() => runWindowCommand("window.close", () => getCurrentWindow().close())}
          >
            <X size={16} />
          </button>
        </div>
      )}
    </header>
  );
}
