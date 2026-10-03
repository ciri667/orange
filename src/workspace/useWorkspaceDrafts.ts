import { useRef, useState } from "react";
import type { EditorFileTab, Note, WorkspaceDocument, WorkspaceSnapshot } from "../shared/types";

/** 进入本次编辑前的 Markdown 正文，放弃更改时写回快照。 */
interface CleanNoteBody {
  content: string;
  tags: string[];
  updatedAt: string;
  contentHash: string;
}

/** 进入本次编辑前的 TXT 正文，放弃更改时写回快照。 */
interface CleanDocumentBody {
  content?: string;
  contentHash: string;
}

/** 为笔记建立当前文件 hash 映射，保存草稿时用于外部修改冲突校验。 */
function buildNoteHashMap(notes: Note[]) {
  return Object.fromEntries(notes.map((note) => [note.id, note.contentHash]));
}

/** 为普通文档建立当前文件 hash 映射，保存 txt 草稿时用于外部修改冲突校验。 */
function buildDocumentHashMap(documents: WorkspaceDocument[]) {
  return Object.fromEntries(documents.map((document) => [document.id, document.contentHash]));
}

/** 管理 Markdown/TXT 草稿 dirty 状态和保存基准 hash，不直接执行文件系统写入。 */
export function useWorkspaceDrafts() {
  /** Markdown 文件开始编辑时的基准 hash，用于保存前冲突检测。 */
  const [editingBaseHashes, setEditingBaseHashes] = useState<Record<string, string>>({});
  /** TXT 文档开始编辑时的基准 hash，用于保存前冲突检测。 */
  const [editingBaseDocumentHashes, setEditingBaseDocumentHashes] = useState<Record<string, string>>({});
  /** 当前存在未保存草稿的 Markdown ID 集合。 */
  const [dirtyNoteIds, setDirtyNoteIds] = useState<Set<string>>(new Set());
  /** 当前存在未保存草稿的 TXT 文档 ID 集合。 */
  const [dirtyDocumentIds, setDirtyDocumentIds] = useState<Set<string>>(new Set());
  /** 仅保存仍处于草稿中的文件的编辑前正文，避免把整库内容再复制一份。 */
  const cleanNoteBodiesRef = useRef(new Map<string, CleanNoteBody>());
  /** 仅保存仍处于草稿中的 TXT 的编辑前正文。 */
  const cleanDocumentBodiesRef = useRef(new Map<string, CleanDocumentBody>());

  /** 在第一次改动落进快照前记住干净正文；已记住的草稿不会被后续按键覆盖。 */
  function rememberCleanNote(note: Note) {
    if (cleanNoteBodiesRef.current.has(note.id)) {
      return;
    }

    cleanNoteBodiesRef.current.set(note.id, {
      content: note.content,
      tags: [...note.tags],
      updatedAt: note.updatedAt,
      contentHash: note.contentHash,
    });
  }

  /** 在第一次改动落进快照前记住干净 TXT 正文。 */
  function rememberCleanDocument(document: WorkspaceDocument) {
    if (cleanDocumentBodiesRef.current.has(document.id)) {
      return;
    }

    cleanDocumentBodiesRef.current.set(document.id, {
      content: document.content,
      contentHash: document.contentHash,
    });
  }

  /** 文件已保存、放弃或从快照消失后，编辑前正文不再需要。 */
  function releaseCleanBodies(snapshot: WorkspaceSnapshot, dirtyNotes: Set<string>, dirtyDocuments: Set<string>) {
    const noteIds = new Set(snapshot.notes.map((note) => note.id));
    const documentIds = new Set(snapshot.documents.map((document) => document.id));

    cleanNoteBodiesRef.current.forEach((_body, noteId) => {
      if (!noteIds.has(noteId) || !dirtyNotes.has(noteId)) {
        cleanNoteBodiesRef.current.delete(noteId);
      }
    });
    cleanDocumentBodiesRef.current.forEach((_body, documentId) => {
      if (!documentIds.has(documentId) || !dirtyDocuments.has(documentId)) {
        cleanDocumentBodiesRef.current.delete(documentId);
      }
    });
  }

  /**
   * 放弃一个文件的未保存草稿。
   *
   * 返回的快照正文和 dirty 集合必须交给同一次 commitSnapshot。
   * 分开 setState 时，后续提交会用调用瞬间仍包含该文件的旧 dirty 集合把标记写回去。
   */
  function discardWorkspaceDraft(
    snapshot: WorkspaceSnapshot,
    tab: EditorFileTab,
    dirtyNotes: Set<string>,
    dirtyDocuments: Set<string>,
  ) {
    const nextDirtyNoteIds = new Set(dirtyNotes);
    const nextDirtyDocumentIds = new Set(dirtyDocuments);

    if (tab.kind === "note") {
      nextDirtyNoteIds.delete(tab.id);
      const saved = cleanNoteBodiesRef.current.get(tab.id);

      if (!saved) {
        return { snapshot, dirtyNoteIds: nextDirtyNoteIds, dirtyDocumentIds: nextDirtyDocumentIds };
      }

      return {
        snapshot: {
          ...snapshot,
          notes: snapshot.notes.map((note) =>
            note.id === tab.id
              ? {
                  ...note,
                  content: saved.content,
                  tags: [...saved.tags],
                  updatedAt: saved.updatedAt,
                  contentHash: saved.contentHash,
                }
              : note,
          ),
        },
        dirtyNoteIds: nextDirtyNoteIds,
        dirtyDocumentIds: nextDirtyDocumentIds,
      };
    }

    nextDirtyDocumentIds.delete(tab.id);
    const saved = cleanDocumentBodiesRef.current.get(tab.id);

    if (!saved) {
      return { snapshot, dirtyNoteIds: nextDirtyNoteIds, dirtyDocumentIds: nextDirtyDocumentIds };
    }

    return {
      snapshot: {
        ...snapshot,
        documents: snapshot.documents.map((document) =>
          document.id === tab.id
            ? { ...document, content: saved.content, contentHash: saved.contentHash }
            : document,
        ),
      },
      dirtyNoteIds: nextDirtyNoteIds,
      dirtyDocumentIds: nextDirtyDocumentIds,
    };
  }

  /** 首屏快照加载完成后初始化保存基准，避免后续保存误判为外部冲突。 */
  function initializeDraftBaselines(snapshot: WorkspaceSnapshot) {
    setEditingBaseHashes(buildNoteHashMap(snapshot.notes));
    setEditingBaseDocumentHashes(buildDocumentHashMap(snapshot.documents));
  }

  /** 快照变更后同步 dirty 集合和基准 hash，调用方负责同时提交 snapshot state。 */
  function commitDraftSnapshot(
    nextSnapshot: WorkspaceSnapshot,
    dirtyNotesToKeep = dirtyNoteIds,
    dirtyDocumentsToKeep = dirtyDocumentIds,
  ) {
    const nextNoteIds = new Set(nextSnapshot.notes.map((note) => note.id));
    const nextDirtyNoteIds = new Set(Array.from(dirtyNotesToKeep).filter((noteId) => nextNoteIds.has(noteId)));
    const nextDocumentIds = new Set(nextSnapshot.documents.map((document) => document.id));
    const nextDirtyDocumentIds = new Set(
      Array.from(dirtyDocumentsToKeep).filter((documentId) => nextDocumentIds.has(documentId)),
    );

    setEditingBaseHashes((currentHashes) => {
      const nextHashes = { ...currentHashes };

      // 新增或成功保存后的笔记需要更新保存基准；仍处于草稿状态的笔记保留原始 hash 用于冲突校验。
      nextSnapshot.notes.forEach((note) => {
        if (!nextDirtyNoteIds.has(note.id)) {
          nextHashes[note.id] = note.contentHash;
        } else if (!nextHashes[note.id]) {
          nextHashes[note.id] = note.contentHash;
        }
      });

      Object.keys(nextHashes).forEach((noteId) => {
        if (!nextNoteIds.has(noteId)) {
          delete nextHashes[noteId];
        }
      });

      return nextHashes;
    });
    setEditingBaseDocumentHashes((currentHashes) => {
      const nextHashes = { ...currentHashes };

      // TXT 文档保存成功或重扫后更新基准 hash；仍在编辑的文档保留原始 hash 做冲突检测。
      nextSnapshot.documents.forEach((document) => {
        if (!nextDirtyDocumentIds.has(document.id)) {
          nextHashes[document.id] = document.contentHash;
        } else if (!nextHashes[document.id]) {
          nextHashes[document.id] = document.contentHash;
        }
      });

      Object.keys(nextHashes).forEach((documentId) => {
        if (!nextDocumentIds.has(documentId)) {
          delete nextHashes[documentId];
        }
      });

      return nextHashes;
    });
    releaseCleanBodies(nextSnapshot, nextDirtyNoteIds, nextDirtyDocumentIds);
    setDirtyNoteIds(nextDirtyNoteIds);
    setDirtyDocumentIds(nextDirtyDocumentIds);
  }

  return {
    editingBaseHashes,
    editingBaseDocumentHashes,
    dirtyNoteIds,
    setDirtyNoteIds,
    dirtyDocumentIds,
    setDirtyDocumentIds,
    initializeDraftBaselines,
    commitDraftSnapshot,
    rememberCleanNote,
    rememberCleanDocument,
    discardWorkspaceDraft,
  };
}
