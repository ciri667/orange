import {
  ChevronDown,
  ChevronRight,
  File,
  FileImage,
  FilePenLine,
  FileText,
  FileType,
  FolderOpen,
  FolderPlus,
  History,
  MoreHorizontal,
  Plus,
  Trash2,
} from "lucide-react";
import { useState, type ComponentProps } from "react";
import { Button } from "../shared/Button";
import { cn } from "../shared/cn";
import { logDebug } from "../shared/logger";
import { Menu, MenuItem, MenuPanel, type MenuPanelPlacement } from "../shared/Menu";
import { OverflowTooltipText } from "../shared/OverflowTooltipText";
import type { FileTreeNode } from "../shared/types";

/** 文件树行的共用外观；active 只用于当前打开的文件。 */
function fileTreeRowClassName({ isRoot = false, isActive = false }: { isRoot?: boolean; isActive?: boolean } = {}) {
  return cn(
    "group relative flex min-h-8 min-w-0 w-full items-center gap-[7px] overflow-visible rounded-control border border-transparent bg-transparent pr-2 text-left text-ink",
    !isActive && "hover:bg-surface-hover",
    isRoot && "border-border-translucent bg-warm-panel",
    isActive && "border-primary-border bg-primary-wash text-agent-strong",
  );
}

/** 本地文件树组件，递归展示文件夹、Markdown、txt、docx、pdf 和图片文件。 */
export function FileTree({
  nodes,
  activeNoteId,
  activeDocumentId,
  collapsedFolderPaths,
  depth = 0,
  isFiltered = false,
  onToggleFolder,
  onSelectNote,
  onSelectDocument,
  onRenameNote,
  onDeleteNote,
  onOpenNoteHistory,
  onRenameDocument,
  onDeleteDocument,
  onOpenDocumentHistory,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  nodes: FileTreeNode[];
  activeNoteId: string;
  activeDocumentId: string;
  collapsedFolderPaths: Set<string>;
  depth?: number;
  isFiltered?: boolean;
  onToggleFolder: (folderPath: string) => void;
  onSelectNote: (noteId: string) => void;
  onSelectDocument: (documentId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onOpenNoteHistory: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
  onOpenDocumentHistory: (documentId: string) => void;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  // 同一时刻只允许一个文件夹行的新建菜单或文件行的操作菜单展开，路径为空表示全部收起。
  const [openCreateMenuPath, setOpenCreateMenuPath] = useState<string | null>(null);
  const [openFileActionPath, setOpenFileActionPath] = useState<string | null>(null);

  /** 切换文件行的低频操作菜单，日志只记录文件类型和菜单状态。 */
  function handleToggleFileActionMenu(node: FileTreeNode) {
    const nextOpenState = openFileActionPath !== node.path;

    logDebug("切换文件树低频操作菜单。", {
      category: "frontend",
      event: "file_tree_action_menu_toggle",
      status: nextOpenState ? "opened" : "closed",
      metadata: {
        nodeType: node.type,
        fileType: node.fileType,
        depth,
      },
    });
    setOpenFileActionPath(nextOpenState ? node.path : null);
    setOpenCreateMenuPath(null);
  }

  // 空树仍要保留根目录新建入口；否则没有文件的知识库会只剩一句提示、无法创建。
  if (!nodes.length && depth === 0) {
    return (
      <EmptyFolderCreateHint
        isFiltered={isFiltered}
        folderPath=""
        onCreateMarkdown={onCreateMarkdown}
        onCreateText={onCreateText}
        onCreateFolder={onCreateFolder}
      />
    );
  }

  return (
    <ul className="m-0 min-w-0 list-none p-0" role={depth === 0 ? "tree" : "group"}>
      {nodes.map((node) => {
        const isCollapsed = collapsedFolderPaths.has(node.path);

        // 文件夹节点只控制展开状态，不直接打开笔记。
        if (node.type === "folder") {
          return (
            <li key={node.id}>
              <div className={fileTreeRowClassName({ isRoot: Boolean(node.isRoot) })} style={{ paddingLeft: depth * 14 + 6 }}>
                <button
                  className="inline-flex min-h-7 min-w-0 flex-1 items-center gap-[7px] border-0 bg-transparent p-0 text-left text-inherit"
                  type="button"
                  aria-expanded={!isCollapsed}
                  aria-label={`${isCollapsed ? "展开" : "收起"} ${node.name}`}
                  onClick={() => onToggleFolder(node.path)}
                >
                  {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                  <FolderOpen size={15} />
                  <OverflowTooltipText className="min-w-0 flex-1 truncate" text={node.name} logArea="file_tree_folder" />
                </button>
                <span className="text-[11px] text-ink-soft">{node.children.length}</span>
                <div className="relative inline-flex shrink-0 overflow-visible">
                  <CreateMenu
                    isOpen={openCreateMenuPath === node.path}
                    placement={node.isRoot ? "bottom-end" : "top-end"}
                    onToggle={() => {
                      setOpenCreateMenuPath(openCreateMenuPath === node.path ? null : node.path);
                      setOpenFileActionPath(null);
                    }}
                    onClose={() => setOpenCreateMenuPath(null)}
                    folderName={node.name}
                    folderPath={node.path}
                    onCreateMarkdown={onCreateMarkdown}
                    onCreateText={onCreateText}
                    onCreateFolder={onCreateFolder}
                  />
                </div>
              </div>
              {!isCollapsed &&
                (node.children.length ? (
                  <FileTree
                    nodes={node.children}
                    activeNoteId={activeNoteId}
                    activeDocumentId={activeDocumentId}
                    collapsedFolderPaths={collapsedFolderPaths}
                    depth={depth + 1}
                    isFiltered={isFiltered}
                    onToggleFolder={onToggleFolder}
                    onSelectNote={onSelectNote}
                    onSelectDocument={onSelectDocument}
                    onRenameNote={onRenameNote}
                    onDeleteNote={onDeleteNote}
                    onOpenNoteHistory={onOpenNoteHistory}
                    onRenameDocument={onRenameDocument}
                    onDeleteDocument={onDeleteDocument}
                    onOpenDocumentHistory={onOpenDocumentHistory}
                    onCreateMarkdown={onCreateMarkdown}
                    onCreateText={onCreateText}
                    onCreateFolder={onCreateFolder}
                  />
                ) : node.isRoot ? (
                  <EmptyFolderCreateHint
                    isFiltered={isFiltered}
                    folderPath={node.path}
                    onCreateMarkdown={onCreateMarkdown}
                    onCreateText={onCreateText}
                    onCreateFolder={onCreateFolder}
                  />
                ) : null)}
            </li>
          );
        }

        const noteId = node.noteId;
        const documentId = node.documentId;
        const isActiveFile = noteId === activeNoteId || documentId === activeDocumentId;
        const canOpenHistory = Boolean(node.capabilities?.canEdit);
        const canRename = Boolean(node.capabilities?.canRename);
        const canDelete = Boolean(node.capabilities?.canDelete);

        return (
          <li key={node.id}>
            <div
              className={fileTreeRowClassName({ isActive: isActiveFile })}
              style={{ paddingLeft: depth * 14 + 28 }}
              role="treeitem"
              aria-selected={isActiveFile}
            >
              <button
                className="inline-flex min-h-7 min-w-0 flex-1 items-center gap-[7px] border-0 bg-transparent p-0 text-left text-inherit"
                type="button"
                aria-label={`打开 ${node.name}`}
                onClick={() => {
                  if (noteId) {
                    onSelectNote(noteId);
                  } else if (documentId) {
                    onSelectDocument(documentId);
                  }
                }}
              >
                <FileTreeIcon node={node} />
                <OverflowTooltipText className="min-w-0 flex-1 truncate" text={node.name} logArea="file_tree_file" />
                <span className="shrink-0 rounded-full bg-[rgba(241,238,232,0.9)] px-[5px] py-px text-[10px] font-bold text-ink-soft">
                  {formatFileTreeTypeLabel(node)}
                </span>
              </button>
              {(canOpenHistory || canRename || canDelete) && (
                <div className="relative inline-flex shrink-0 overflow-visible">
                  <FileActionMenu
                    isOpen={openFileActionPath === node.path}
                    onToggle={() => handleToggleFileActionMenu(node)}
                    onClose={() => setOpenFileActionPath(null)}
                    canOpenHistory={canOpenHistory}
                    canRename={canRename}
                    canDelete={canDelete}
                    noteId={noteId}
                    documentId={documentId}
                    onOpenNoteHistory={onOpenNoteHistory}
                    onOpenDocumentHistory={onOpenDocumentHistory}
                    onRenameNote={onRenameNote}
                    onDeleteNote={onDeleteNote}
                    onRenameDocument={onRenameDocument}
                    onDeleteDocument={onDeleteDocument}
                  />
                </div>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}

/** 空根目录的可见新建入口，避免只依赖可能被滚动容器裁切的「+」下拉菜单。 */
function EmptyFolderCreateHint({
  isFiltered,
  folderPath,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  isFiltered: boolean;
  folderPath: string;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  return (
    <div className="mx-1 my-2 rounded-lg border border-dashed border-border bg-surface-translucent p-3">
      <p className="mb-2.5 mt-0 text-[13px] text-ink-muted">
        {isFiltered ? "没有匹配的支持文档" : "当前知识库还没有文件，可以在根目录新建。"}
      </p>
      <div className="flex flex-wrap gap-1.5">
        <Button variant="ghost" size="compact" onClick={() => onCreateMarkdown(folderPath)}>
          <FileText size={14} />
          新建 Markdown
        </Button>
        <Button variant="ghost" size="compact" onClick={() => onCreateText(folderPath)}>
          <FileType size={14} />
          新建 TXT
        </Button>
        <Button variant="ghost" size="compact" onClick={() => onCreateFolder(folderPath)}>
          <FolderPlus size={14} />
          新建目录
        </Button>
      </div>
    </div>
  );
}

/** 文件夹行的「+」按钮与新建菜单。点击树内别处也会关闭，避免旧菜单残留。 */
function CreateMenu({
  isOpen,
  placement,
  onToggle,
  onClose,
  folderName,
  folderPath,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  isOpen: boolean;
  placement: MenuPanelPlacement;
  onToggle: () => void;
  onClose: () => void;
  folderName: string;
  folderPath: string;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  return (
    <Menu open={isOpen} onClose={onClose}>
      <FileTreeMenuTrigger
        isOpen={isOpen}
        aria-label={`在「${folderName}」中新建`}
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        <Plus size={14} />
      </FileTreeMenuTrigger>
      {isOpen && (
        <MenuPanel placement={placement}>
          <MenuItem
            onClick={() => {
              onClose();
              onCreateMarkdown(folderPath);
            }}
          >
            <FileText size={14} />
            新建 Markdown
          </MenuItem>
          <MenuItem
            onClick={() => {
              onClose();
              onCreateText(folderPath);
            }}
          >
            <FileType size={14} />
            新建 TXT
          </MenuItem>
          <MenuItem
            onClick={() => {
              onClose();
              onCreateFolder(folderPath);
            }}
          >
            <FolderPlus size={14} />
            新建目录
          </MenuItem>
        </MenuPanel>
      )}
    </Menu>
  );
}

/** 文件行的「更多操作」按钮与菜单。点树内其它行也会关闭。 */
function FileActionMenu({
  isOpen,
  onToggle,
  onClose,
  canOpenHistory,
  canRename,
  canDelete,
  noteId,
  documentId,
  onOpenNoteHistory,
  onOpenDocumentHistory,
  onRenameNote,
  onDeleteNote,
  onRenameDocument,
  onDeleteDocument,
}: {
  isOpen: boolean;
  onToggle: () => void;
  onClose: () => void;
  canOpenHistory: boolean;
  canRename: boolean;
  canDelete: boolean;
  noteId: string | undefined;
  documentId: string | undefined;
  onOpenNoteHistory: (noteId: string) => void;
  onOpenDocumentHistory: (documentId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
}) {
  return (
    <Menu open={isOpen} onClose={onClose}>
      <FileTreeMenuTrigger
        isOpen={isOpen}
        title="更多文件操作"
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        <MoreHorizontal size={14} />
      </FileTreeMenuTrigger>
      {isOpen && (
        <MenuPanel placement="top-end">
          {canOpenHistory && (
            <MenuItem
              onClick={() => {
                onClose();
                if (noteId) {
                  onOpenNoteHistory(noteId);
                } else if (documentId) {
                  onOpenDocumentHistory(documentId);
                }
              }}
            >
              <History size={14} />
              历史记录
            </MenuItem>
          )}
          {canRename && (
            <MenuItem
              onClick={() => {
                onClose();
                if (noteId) {
                  onRenameNote(noteId);
                } else if (documentId) {
                  onRenameDocument(documentId);
                }
              }}
            >
              <FilePenLine size={14} />
              重命名
            </MenuItem>
          )}
          {canDelete && (
            <MenuItem
              tone="danger"
              onClick={() => {
                onClose();
                if (noteId) {
                  onDeleteNote(noteId);
                } else if (documentId) {
                  onDeleteDocument(documentId);
                }
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

/** 文件树行内的小图标触发器，默认半透明，悬停行或展开菜单时完全显示。 */
function FileTreeMenuTrigger({
  isOpen,
  children,
  ...props
}: ComponentProps<typeof Button> & { isOpen: boolean }) {
  return (
    <Button
      variant="icon"
      size="compact"
      aria-haspopup="menu"
      aria-expanded={isOpen}
      {...props}
      className={
        isOpen
          ? "border-border-strong bg-surface-hover text-ink opacity-100"
          : "border-transparent bg-transparent text-ink-soft opacity-[0.58] group-hover:opacity-100 hover:enabled:border-border-strong hover:enabled:bg-surface-hover hover:enabled:text-ink"
      }
    >
      {children}
    </Button>
  );
}

/** 根据文件类型选择目录树图标，帮助用户快速区分编辑和预览文档。 */
function FileTreeIcon({ node }: { node: FileTreeNode }) {
  if (node.fileType === "txt") {
    return <FileType size={14} />;
  }

  if (node.fileType === "docx" || node.fileType === "pdf") {
    return <File size={14} />;
  }

  if (node.fileType === "image") {
    return <FileImage size={14} />;
  }

  return <FileText size={14} />;
}

/** 将文件类型转为短标签，作为侧栏扫描时的弱信息。 */
function formatFileTreeTypeLabel(node: FileTreeNode) {
  if (node.fileType === "txt") {
    return "TXT";
  }

  if (node.fileType === "docx") {
    return "DOCX";
  }

  if (node.fileType === "pdf") {
    return "PDF";
  }

  if (node.fileType === "image") {
    return "IMG";
  }

  return "MD";
}
