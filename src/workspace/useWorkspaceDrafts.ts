import { useState } from "react";
import type { Note, WorkspaceDocument, WorkspaceSnapshot } from "../shared/types";

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
  };
}
