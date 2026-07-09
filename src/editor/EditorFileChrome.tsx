import { ChevronDown, FileDown, FilePenLine, History, MoreHorizontal, Trash2 } from "lucide-react";
import { useState, type ReactNode } from "react";
import { logDebug } from "../shared/logger";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import { useDismissable } from "../shared/useDismissable";
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
    <header className="editor-header">
      <div>
        <OverflowTooltipText as="p" className="path-label" text={title.pathLabel} logArea={title.pathLogArea} />
        <OverflowTooltipText as="h2" text={title.title} logArea={title.titleLogArea} />
      </div>
      <div className="editor-actions">{actions}</div>
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
    <header className="editor-header">
      <div>
        <OverflowTooltipText as="p" className="path-label" text={pathLabel} logArea={pathLogArea} />
        <h2>{title}</h2>
      </div>
      <div className="editor-actions" />
    </header>
  );
}

/** 编辑器元信息条，复用保存状态、阅读统计和文档类型的紧凑展示。 */
export function EditorMetaStrip({ items }: { items: EditorMetaItem[] }) {
  return (
    <div className="meta-strip">
      {items.map((item, index) => (
        <span className={item.className} key={index}>
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
  /** 点击菜单以外区域或按 Esc 时关闭菜单，和旧实现保持一致。 */
  const moreMenuRef = useDismissable<HTMLDivElement>(isMoreMenuOpen, () => setIsMoreMenuOpen(false));

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
    <div className="more-menu-wrapper" ref={moreMenuRef}>
      <button
        className="icon-button"
        type="button"
        title="更多文件操作"
        aria-haspopup="menu"
        aria-expanded={isMoreMenuOpen}
        onClick={handleMoreMenuToggle}
        disabled={isBusy}
      >
        <MoreHorizontal size={18} />
      </button>
      {isMoreMenuOpen && (
        <div className="more-action-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            aria-haspopup="menu"
            aria-expanded={isExportMenuOpen}
            onClick={() => setIsExportMenuOpen((isOpen) => !isOpen)}
          >
            <FileDown size={14} />
            导出当前文件
            <ChevronDown size={13} />
          </button>
          {isExportMenuOpen &&
            exportOptions.map((option) => (
              <button
                className="nested-menu-item"
                key={option.format}
                type="button"
                role="menuitem"
                onClick={() => {
                  setIsExportMenuOpen(false);
                  setIsMoreMenuOpen(false);
                  void onExportFile(option.format);
                }}
              >
                <FileDown size={14} />
                {option.label}
              </button>
            ))}
          {onOpenHistory && (
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setIsMoreMenuOpen(false);
                onOpenHistory();
              }}
            >
              <History size={14} />
              历史记录
            </button>
          )}
          {onRename && (
            <button
              type="button"
              role="menuitem"
              onClick={() => {
                setIsMoreMenuOpen(false);
                onRename();
              }}
            >
              <FilePenLine size={14} />
              重命名
            </button>
          )}
          {onDelete && (
            <button
              className="danger"
              type="button"
              role="menuitem"
              onClick={() => {
                setIsMoreMenuOpen(false);
                onDelete();
              }}
            >
              <Trash2 size={14} />
              删除
            </button>
          )}
        </div>
      )}
    </div>
  );
}
