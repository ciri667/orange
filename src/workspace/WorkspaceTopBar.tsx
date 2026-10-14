import { Fragment, useState } from "react";
import { PanelLeftClose } from "lucide-react";
import { Button } from "../shared/Button";
import { MenuItem, MenuPanel } from "../shared/Menu";
import { cn } from "../shared/cn";
import { useDismissable } from "../shared/useDismissable";

/**
 * 顶栏一个下拉菜单的声明。
 *
 * 菜单项目前只做占位（`disabled`），等具体操作项定下来后再逐条接真实动作；
 * 这里先固定菜单骨架，避免后续接动作时再调整顶栏结构。
 */
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
    <div ref={containerRef} className="relative inline-flex">
      <button
        type="button"
        className={cn(
          "inline-flex min-h-7 items-center rounded-small border-0 bg-transparent px-2.5 text-[13px] text-ink-muted",
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
 * 工作台顶栏。
 *
 * 收起/展开知识库栏的唯一入口：面板内不再有就地收起按钮，
 * 收起后也只需点这一个图标，不用再悬浮图标栏、从浮层里找「展开」。
 *
 * 菜单目前是占位骨架（条目禁用），具体操作项后续再逐条接入。
 */
export function WorkspaceTopBar({ sidebarOpen, onToggleSidebar }: WorkspaceTopBarProps) {
  return (
    <header
      className="flex min-h-11 shrink-0 items-center gap-2 border-b border-border bg-surface px-2"
      aria-label="工作台顶栏"
    >
      <div className="flex shrink-0 items-center gap-2 pl-1">
        <div className="grid size-6 place-items-center overflow-hidden rounded-md">
          <img className="block size-full object-contain" src="/orange-logo.svg" alt="" />
        </div>
        <strong className="text-[14px] font-semibold text-ink-strong">橘记</strong>
      </div>

      {/*
        收展按钮紧跟品牌之后：窄窗口（≤760px）下样式会把侧栏整体 display:none，
        这里跟着一起隐藏，避免出现点了没反应的开关。
      */}
      <Button
        variant="icon"
        className="max-[760px]:hidden"
        title={sidebarOpen ? "收起知识库栏" : "展开知识库栏"}
        aria-label={sidebarOpen ? "收起知识库栏" : "展开知识库栏"}
        aria-pressed={sidebarOpen}
        onClick={onToggleSidebar}
      >
        <PanelLeftClose size={16} />
      </Button>

      <nav className="flex min-w-0 items-center gap-0.5" aria-label="主菜单">
        {TOP_BAR_MENUS.map((menu) => (
          <Fragment key={menu.id}>
            <TopBarMenu menu={menu} />
          </Fragment>
        ))}
      </nav>
    </header>
  );
}
