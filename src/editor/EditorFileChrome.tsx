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
  /** 有点击回调时渲染为按钮，供标签等可交互元信息使用。 */
  onClick?: () => void;
  title?: string;
  ariaLabel?: string;
  ariaExpanded?: boolean;
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
    <header className="editor-file-header">
      <div className="editor-file-topline">
        <OverflowTooltipText as="p" className={pathLabelClassName} text={title.pathLabel} logArea={title.pathLogArea} />
        <div className="editor-file-actions">{actions}</div>
      </div>
      <OverflowTooltipText as="h1" className="editor-file-title" text={title.title} logArea={title.titleLogArea} />
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
    <header className="editor-file-header">
      <OverflowTooltipText as="p" className={pathLabelClassName} text={pathLabel} logArea={pathLogArea} />
      <h1 className="editor-file-title">{title}</h1>
    </header>
  );
}

/** 元信息和辅助操作独立于文件工具栏，窄编辑区内可自然换行。 */
export function EditorMetaStrip({ items, actions }: { items: EditorMetaItem[]; actions?: ReactNode }) {
  return (
    <div className="editor-meta-strip">
      <div className="editor-meta-items">
        {items.map((item, index) => {
          const itemClassName = cn(
            "inline-flex shrink-0 items-center gap-1.5 rounded-small border-0 bg-transparent px-1 py-1 text-xs text-ink-muted",
            item.className === "dirty-indicator" && "bg-warning-soft font-medium text-warning",
            item.className !== "dirty-indicator" && item.className,
            item.onClick && "cursor-pointer hover:border-border-strong hover:bg-surface-hover",
          );

          if (item.onClick) {
            return (
              <button
                className={itemClassName}
                key={index}
                type="button"
                title={item.title}
                aria-label={item.ariaLabel}
                aria-expanded={item.ariaExpanded}
                onClick={item.onClick}
              >
                {item.icon}
                {item.text}
              </button>
            );
          }

          return (
            <span className={itemClassName} key={index}>
              {item.icon}
              {item.text}
            </span>
          );
        })}
      </div>
      {actions && <div className="editor-meta-actions">{actions}</div>}
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
