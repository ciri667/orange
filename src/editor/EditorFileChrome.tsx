import { ChevronDown, FileDown, FilePenLine, History, MoreHorizontal, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { pathLabelClassName } from "../shared/ui";
import { logDebug } from "../shared/logger";
import { Menu, MenuItem, MenuPanel } from "../shared/Menu";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { ExportFormat } from "../shared/types";

/** 编辑器头部标题区入参，统一 Markdown 和普通文档的路径/标题展示。 */
export interface EditorFileHeaderTitle {
  pathLabel: string;
  pathLogArea: string;
  title: string;
  titleLogArea: string;
}

/** 文件导出菜单项，调用方按文件类型提供支持的格式。 */
export interface EditorExportOption {
  format: ExportFormat;
  label: string;
}

/** 编辑器更多菜单日志上下文，只允许脱敏字段和轻量状态。 */
export interface EditorMoreActionLogContext {
  event: string;
  metadata: Record<string, string | number | boolean | undefined>;
}

/** 编辑器元信息条单项，图标由调用方传入以保留现有视觉。 */
export interface EditorMetaItem {
  icon: ReactNode;
  text: ReactNode;
  className?: string;
}

/** 编辑器和文档面板共用头部，避免两套标题 DOM 和截断逻辑分叉。 */
export function EditorFileHeader({
  title,
  actions,
}: {
  title: EditorFileHeaderTitle;
  actions?: ReactNode;
}) {
  return (
    <header className="flex items-start justify-between gap-3">
      <div className="min-w-0">
        <OverflowTooltipText as="p" className={pathLabelClassName} text={title.pathLabel} logArea={title.pathLogArea} />
        <OverflowTooltipText as="h2" className="mt-1 mb-0 truncate text-xl leading-tight text-ink-strong" text={title.title} logArea={title.titleLogArea} />
      </div>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-[7px]">{actions}</div>
    </header>
  );
}

/** 空编辑器头部只展示纯标题，保持空态布局和原实现一致。 */
export function EditorEmptyHeader({
  pathLabel,
  pathLogArea,
  title,
}: {
  pathLabel: string;
  pathLogArea: string;
  title: string;
}) {
  return (
    <header className="flex items-start justify-between gap-3">
      <div className="min-w-0">
        <OverflowTooltipText as="p" className={pathLabelClassName} text={pathLabel} logArea={pathLogArea} />
        <h2 className="mt-1 mb-0 truncate text-xl leading-tight text-ink-strong">{title}</h2>
      </div>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-[7px]" />
    </header>
  );
}

/** 编辑器元信息条，复用保存状态、阅读统计和文档类型的紧凑展示。 */
export function EditorMetaStrip({ items }: { items: EditorMetaItem[] }) {
  return (
    <div className="flex gap-2 overflow-x-auto text-xs text-ink-muted [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {items.map((item, index) => (
        <span
          className={cn(
            "inline-flex shrink-0 items-center gap-1.5 rounded-full border border-[rgba(230,224,214,0.7)] bg-[rgba(251,250,247,0.72)] px-2 py-1",
            item.className === "dirty-indicator" && "border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft font-bold text-warning",
            item.className !== "dirty-indicator" && item.className,
          )}
          key={index}
        >
          {item.icon}
          {item.text}
        </span>
      ))}
    </div>
  );
}

/** Markdown/TXT/只读文档共用的更多操作菜单，内部只持有菜单开关局部状态。 */
export function EditorMoreActionMenu({
  exportOptions,
  isBusy,
  logContext,
  onExportFile,
  onOpenHistory,
  onRename,
  onDelete,
}: {
  exportOptions: EditorExportOption[];
  isBusy: boolean;
  logContext: EditorMoreActionLogContext;
  onExportFile: (format: ExportFormat) => void | Promise<void>;
  onOpenHistory?: () => void;
  onRename?: () => void;
  onDelete?: () => void;
}) {
  /** 导出子菜单开关属于更多菜单内部状态，关闭父菜单时同步清理。 */
  const [isExportMenuOpen, setIsExportMenuOpen] = useState(false);
  /** 更多菜单展开状态只影响当前头部，不进入工作台全局状态。 */
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
  /** 关闭更多菜单时同步收起导出子菜单。 */
  function handleMoreMenuClose() {
    setIsMoreMenuOpen(false);
    setIsExportMenuOpen(false);
  }

  /** 切换低频操作菜单，并写入调用方指定的脱敏事件。 */
  function handleMoreMenuToggle() {
    const nextOpenState = !isMoreMenuOpen;

    logDebug("切换编辑器更多操作菜单。", {
      category: "frontend",
      event: logContext.event,
      status: nextOpenState ? "opened" : "closed",
      metadata: logContext.metadata,
    });
    setIsMoreMenuOpen(nextOpenState);
    setIsExportMenuOpen(false);
  }

  return (
    <Menu open={isMoreMenuOpen} onClose={handleMoreMenuClose}>
      <Button
        variant="icon"
        title="更多文件操作"
        aria-haspopup="menu"
        aria-expanded={isMoreMenuOpen}
        onClick={handleMoreMenuToggle}
        disabled={isBusy}
      >
        <MoreHorizontal size={18} />
      </Button>
      {isMoreMenuOpen && (
        <MenuPanel placement="bottom-end" className="min-w-[172px]">
          <MenuItem
            aria-haspopup="menu"
            aria-expanded={isExportMenuOpen}
            onClick={() => setIsExportMenuOpen((isOpen) => !isOpen)}
          >
            <FileDown size={14} />
            导出当前文件
            <ChevronDown size={13} />
          </MenuItem>
          {isExportMenuOpen &&
            exportOptions.map((option) => (
              <MenuItem
                nested
                key={option.format}
                onClick={() => {
                  handleMoreMenuClose();
                  void onExportFile(option.format);
                }}
              >
                <FileDown size={14} />
                {option.label}
              </MenuItem>
            ))}
          {onOpenHistory && (
            <MenuItem
              onClick={() => {
                handleMoreMenuClose();
                onOpenHistory();
              }}
            >
              <History size={14} />
              历史记录
            </MenuItem>
          )}
          {onRename && (
            <MenuItem
              onClick={() => {
                handleMoreMenuClose();
                onRename();
              }}
            >
              <FilePenLine size={14} />
              重命名
            </MenuItem>
          )}
          {onDelete && (
            <MenuItem
              tone="danger"
              onClick={() => {
                handleMoreMenuClose();
                onDelete();
              }}
            >
              <Trash2 size={14} />
              删除
            </MenuItem>
          )}
        </MenuPanel>
      )}
    </Menu>
  );
}
