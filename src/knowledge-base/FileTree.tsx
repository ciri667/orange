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
  MoreHorizontal,
  Plus,
  Trash2,
} from "lucide-react";
import { useState } from "react";
import { logDebug } from "../shared/logger";
import { useDismissable } from "../shared/useDismissable";
import type { FileTreeNode } from "../shared/types";

/** 本地文件树组件，递归展示文件夹、Markdown、txt、docx、pdf 和图片文件。 */
export function FileTree({
  nodes,
  activeNoteId,
  activeDocumentId,
  collapsedFolderPaths,
  depth = 0,
  onToggleFolder,
  onSelectNote,
  onSelectDocument,
  onRenameNote,
  onDeleteNote,
  onRenameDocument,
  onDeleteDocument,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  nodes: FileTreeNode[];
  activeNoteId: string;
  activeDocumentId: string;
  collapsedFolderPaths: Set<string>;
  depth?: number;
  onToggleFolder: (folderPath: string) => void;
  onSelectNote: (noteId: string) => void;
  onSelectDocument: (documentId: string) => void;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
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

  if (!nodes.length && depth === 0) {
    return <p className="file-tree-empty">没有匹配的支持文档</p>;
  }

  return (
    <ul className={`file-tree-list ${depth === 0 ? "root" : ""}`} role={depth === 0 ? "tree" : "group"}>
      {nodes.map((node) => {
        const isCollapsed = collapsedFolderPaths.has(node.path);

        // 文件夹节点只控制展开状态，不直接打开笔记。
        if (node.type === "folder") {
          return (
            <li key={node.id}>
              <div
                className={`file-tree-row folder ${node.isRoot ? "root-folder" : ""}`}
                style={{ paddingLeft: depth * 14 + 6 }}
              >
                <button
                  className="file-tree-open-button"
                  type="button"
                  aria-expanded={!isCollapsed}
                  onClick={() => onToggleFolder(node.path)}
                >
                  {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                  <FolderOpen size={15} />
                  <span className="file-tree-name">{node.name}</span>
                </button>
                <span className="file-tree-count">{node.children.length}</span>
                <div className="file-tree-actions">
                  <CreateMenu
                    isOpen={openCreateMenuPath === node.path}
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
              {!isCollapsed && (
                <FileTree
                  nodes={node.children}
                  activeNoteId={activeNoteId}
                  activeDocumentId={activeDocumentId}
                  collapsedFolderPaths={collapsedFolderPaths}
                  depth={depth + 1}
                  onToggleFolder={onToggleFolder}
                  onSelectNote={onSelectNote}
                  onSelectDocument={onSelectDocument}
                  onRenameNote={onRenameNote}
                  onDeleteNote={onDeleteNote}
                  onRenameDocument={onRenameDocument}
                  onDeleteDocument={onDeleteDocument}
                  onCreateMarkdown={onCreateMarkdown}
                  onCreateText={onCreateText}
                  onCreateFolder={onCreateFolder}
                />
              )}
            </li>
          );
        }

        const noteId = node.noteId;
        const documentId = node.documentId;
        const isActiveFile = noteId === activeNoteId || documentId === activeDocumentId;
        const canRename = Boolean(node.capabilities?.canRename);
        const canDelete = Boolean(node.capabilities?.canDelete);

        return (
          <li key={node.id}>
            <div
              className={`file-tree-row file ${isActiveFile ? "active" : ""}`}
              style={{ paddingLeft: depth * 14 + 28 }}
              role="treeitem"
              aria-selected={isActiveFile}
            >
              <button
                className="file-tree-open-button"
                type="button"
                title={node.name}
                onClick={() => {
                  if (noteId) {
                    onSelectNote(noteId);
                  } else if (documentId) {
                    onSelectDocument(documentId);
                  }
                }}
              >
                <FileTreeIcon node={node} />
                <span className="file-tree-name">{node.name}</span>
                <span className="file-tree-type">{formatFileTreeTypeLabel(node)}</span>
              </button>
              {(canRename || canDelete) && (
                <div className="file-tree-actions">
                  <FileActionMenu
                    isOpen={openFileActionPath === node.path}
                    onToggle={() => handleToggleFileActionMenu(node)}
                    onClose={() => setOpenFileActionPath(null)}
                    canRename={canRename}
                    canDelete={canDelete}
                    noteId={noteId}
                    documentId={documentId}
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

/** 文件夹行的「+」按钮与新建菜单。ref 与 hook 只覆盖按钮 + 菜单这个小范围，
 * 点击树内别处（其它文件夹 / 文件行）也视为外部点击而关闭，避免旧菜单残留。 */
function CreateMenu({
  isOpen,
  onToggle,
  onClose,
  folderName,
  folderPath,
  onCreateMarkdown,
  onCreateText,
  onCreateFolder,
}: {
  isOpen: boolean;
  onToggle: () => void;
  onClose: () => void;
  folderName: string;
  folderPath: string;
  onCreateMarkdown: (parentPath: string) => void;
  onCreateText: (parentPath: string) => void;
  onCreateFolder: (parentPath: string) => void;
}) {
  // ref 仅包住触发按钮与浮层，判定点外部（含树内其它行）即关闭。
  const containerRef = useDismissable<HTMLDivElement>(isOpen, onClose);

  return (
    <div ref={containerRef}>
      <button
        className="file-action-button"
        type="button"
        title={`在「${folderName}」中新建`}
        aria-haspopup="menu"
        aria-expanded={isOpen}
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        <Plus size={14} />
      </button>
      {isOpen && (
        <div className="create-action-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onClose();
              onCreateMarkdown(folderPath);
            }}
          >
            <FileText size={14} />
            新建 Markdown
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onClose();
              onCreateText(folderPath);
            }}
          >
            <FileType size={14} />
            新建 TXT
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onClose();
              onCreateFolder(folderPath);
            }}
          >
            <FolderPlus size={14} />
            新建目录
          </button>
        </div>
      )}
    </div>
  );
}

/** 文件行的「更多操作」按钮与菜单。ref 覆盖按钮 + 菜单，点树内其它行也会关闭。 */
function FileActionMenu({
  isOpen,
  onToggle,
  onClose,
  canRename,
  canDelete,
  noteId,
  documentId,
  onRenameNote,
  onDeleteNote,
  onRenameDocument,
  onDeleteDocument,
}: {
  isOpen: boolean;
  onToggle: () => void;
  onClose: () => void;
  canRename: boolean;
  canDelete: boolean;
  noteId: string | undefined;
  documentId: string | undefined;
  onRenameNote: (noteId: string) => void;
  onDeleteNote: (noteId: string) => void;
  onRenameDocument: (documentId: string) => void;
  onDeleteDocument: (documentId: string) => void;
}) {
  const containerRef = useDismissable<HTMLDivElement>(isOpen, onClose);

  return (
    <div ref={containerRef}>
      <button
        className="file-action-button"
        type="button"
        title="更多文件操作"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        onClick={(event) => {
          event.stopPropagation();
          onToggle();
        }}
      >
        <MoreHorizontal size={14} />
      </button>
      {isOpen && (
        <div className="file-action-menu" role="menu">
          {canRename && (
            <button
              type="button"
              role="menuitem"
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
            </button>
          )}
          {canDelete && (
            <button
              className="danger"
              type="button"
              role="menuitem"
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
            </button>
          )}
        </div>
      )}
    </div>
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
