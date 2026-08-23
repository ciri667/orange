import { History, RotateCcw, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { buildMarkdownDiff } from "../diff/markdownDiff";
import {
  diffGutterClassName,
  diffHunkHeaderClassName,
  diffLineGridClassName,
  diffLineToneClassName,
  unifiedDiffFileClassName,
} from "../diff/diffStyles";
import { Button } from "../shared/Button";
import { ConfirmDialog, type ConfirmDialogConfig } from "../shared/ConfirmDialog";
import { cn } from "../shared/cn";
import { ModalBackdrop, ModalHeader, ModalPanel } from "../shared/Modal";
import { sectionLabelClassName } from "../shared/ui";
import {
  clearDocumentHistory,
  loadDocumentHistory,
  loadDocumentHistoryEntry,
  restoreDocumentHistoryEntry,
} from "../shared/tauriApi";
import type {
  DocumentHistoryEntry,
  DocumentHistoryEntryDetail,
  DocumentHistorySource,
  DocumentHistoryTargetKind,
  WorkspaceSnapshot,
} from "../shared/types";

/** 文档历史弹窗入参；恢复成功后由父级提交新快照和草稿基线。 */
interface DocumentHistoryDialogProps {
  snapshot: WorkspaceSnapshot;
  targetKind: DocumentHistoryTargetKind;
  targetId: string;
  title: string;
  currentContent: string;
  currentHash: string;
  isDirty: boolean;
  isBusy: boolean;
  onClose: () => void;
  onRestored: (snapshot: WorkspaceSnapshot) => void;
  onNotice: (message: string) => void;
}

/** 待确认的历史操作，复用全局 ConfirmDialog 视觉和二次确认语义。 */
interface HistoryConfirmation extends ConfirmDialogConfig {
  onConfirm: () => Promise<void> | void;
}

/** Markdown/TXT 历史记录弹窗，负责列表、详情 diff、恢复和清空当前文件历史。 */
export function DocumentHistoryDialog({
  snapshot,
  targetKind,
  targetId,
  title,
  currentContent,
  currentHash,
  isDirty,
  isBusy,
  onClose,
  onRestored,
  onNotice,
}: DocumentHistoryDialogProps) {
  /** 历史列表只保存元数据，正文详情按选中项延迟加载。 */
  const [entries, setEntries] = useState<DocumentHistoryEntry[]>([]);
  /** 当前选中的历史正文详情，用于生成恢复 diff。 */
  const [selectedDetail, setSelectedDetail] = useState<DocumentHistoryEntryDetail | null>(null);
  /** 加载状态覆盖列表和详情读取，避免重复点击触发并发请求。 */
  const [isLoading, setIsLoading] = useState(false);
  /** 恢复或清空操作状态；和外部 isBusy 合并控制按钮可用性。 */
  const [isWorking, setIsWorking] = useState(false);
  /** 局部错误文案，不包含正文或绝对路径。 */
  const [errorMessage, setErrorMessage] = useState("");
  /** 二次确认弹窗配置，由恢复和清空两个危险动作复用。 */
  const [confirmation, setConfirmation] = useState<HistoryConfirmation | null>(null);
  const isActionDisabled = isBusy || isWorking || isLoading;
  const diff = useMemo(
    () => (selectedDetail ? buildMarkdownDiff(currentContent, selectedDetail.content) : null),
    [currentContent, selectedDetail],
  );

  useEffect(() => {
    let isMounted = true;

    async function loadEntries() {
      setIsLoading(true);
      setErrorMessage("");

      try {
        const nextEntries = await loadDocumentHistory(snapshot, targetKind, targetId);

        if (!isMounted) {
          return;
        }

        setEntries(nextEntries);
        if (nextEntries[0]) {
          await handleSelectEntry(nextEntries[0].id);
        } else {
          setSelectedDetail(null);
        }
      } catch (error) {
        if (isMounted) {
          setErrorMessage(formatDialogError(error));
        }
      } finally {
        if (isMounted) {
          setIsLoading(false);
        }
      }
    }

    void loadEntries();

    return () => {
      isMounted = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snapshot, targetKind, targetId]);

  /** 读取用户点击的历史详情；正文只在这里进入前端内存。 */
  async function handleSelectEntry(entryId: string) {
    setErrorMessage("");

    try {
      const detail = await loadDocumentHistoryEntry(entryId);

      setSelectedDetail(detail);
    } catch (error) {
      setSelectedDetail(null);
      setErrorMessage(formatDialogError(error));
    }
  }

  /** 恢复前弹出二次确认；未保存草稿时按钮已禁用。 */
  function requestRestore() {
    if (!selectedDetail || isDirty) {
      return;
    }

    setConfirmation({
      title: "恢复历史版本",
      message: `恢复「${title}」到 ${selectedDetail.createdAt} 的版本？当前版本会先保存为历史记录。`,
      confirmLabel: "恢复此版本",
      tone: "danger",
      onConfirm: handleRestoreSelected,
    });
  }

  /** 执行恢复写入，并用后端返回快照刷新父级工作台。 */
  async function handleRestoreSelected() {
    if (!selectedDetail) {
      return;
    }

    setConfirmation(null);
    setIsWorking(true);
    setErrorMessage("");

    try {
      const nextSnapshot = await restoreDocumentHistoryEntry(snapshot, selectedDetail.id, currentHash);
      const nextEntries = await loadDocumentHistory(nextSnapshot, targetKind, targetId);

      onRestored(nextSnapshot);
      setEntries(nextEntries);
      setSelectedDetail(nextEntries[0] ? await loadDocumentHistoryEntry(nextEntries[0].id) : null);
      onNotice("已恢复历史版本。");
    } catch (error) {
      const message = formatDialogError(error);

      setErrorMessage(message);
      onNotice(message);
    } finally {
      setIsWorking(false);
    }
  }

  /** 清空只作用于当前文件历史，不删除用户文档。 */
  function requestClear() {
    setConfirmation({
      title: "清空历史记录",
      message: `清空「${title}」的全部历史记录？当前文档不会被删除。`,
      confirmLabel: "清空历史",
      tone: "danger",
      onConfirm: handleClearHistory,
    });
  }

  /** 执行当前文件历史清空，并清理本地详情状态。 */
  async function handleClearHistory() {
    setConfirmation(null);
    setIsWorking(true);
    setErrorMessage("");

    try {
      await clearDocumentHistory(snapshot, targetKind, targetId);
      setEntries([]);
      setSelectedDetail(null);
      onNotice("已清空当前文件历史。");
    } catch (error) {
      const message = formatDialogError(error);

      setErrorMessage(message);
      onNotice(message);
    } finally {
      setIsWorking(false);
    }
  }

  return (
    <>
      <ModalBackdrop onClose={onClose}>
        <ModalPanel
          className="h-[min(760px,calc(100vh-40px))] w-[min(980px,calc(100vw-40px))] grid-rows-[auto_minmax(0,1fr)_auto] max-[980px]:w-[min(920px,calc(100vw-24px))] max-[760px]:h-[calc(100vh-20px)] max-[760px]:w-[min(100%,calc(100vw-20px))]"
          aria-labelledby="history-dialog-title"
        >
          <ModalHeader className="px-4 py-3.5">
            <div className="min-w-0">
              <p className={sectionLabelClassName}>历史记录</p>
              <h2 id="history-dialog-title" className="mt-1 mb-0 text-lg leading-tight text-ink-strong">
                {title}
              </h2>
            </div>
            <Button variant="icon" title="关闭历史记录" onClick={onClose} disabled={isWorking}>
              <X size={18} />
            </Button>
          </ModalHeader>

          <div className="grid min-h-0 grid-cols-[260px_minmax(0,1fr)] max-[980px]:grid-cols-[220px_minmax(0,1fr)] max-[760px]:grid-cols-1 max-[760px]:grid-rows-[minmax(150px,32%)_minmax(0,1fr)]">
            <aside className="min-h-0 overflow-auto border-r border-border bg-warm-panel max-[760px]:border-r-0 max-[760px]:border-b" aria-label="历史版本">
              {isLoading && <p className="m-3 text-[13px] leading-normal text-ink-muted">正在加载...</p>}
              {!isLoading && !entries.length && <p className="m-3 text-[13px] leading-normal text-ink-muted">暂无历史记录</p>}
              {entries.map((entry) => (
                <button
                  className={cn(
                    "grid w-full gap-1 border-0 border-b border-l-[3px] border-b-border-soft border-l-transparent bg-transparent px-3 py-[11px] text-left text-ink hover:border-l-accent hover:bg-accent-soft",
                    selectedDetail?.id === entry.id && "border-l-accent bg-accent-soft",
                  )}
                  key={entry.id}
                  type="button"
                  onClick={() => void handleSelectEntry(entry.id)}
                  disabled={isActionDisabled}
                >
                  <span className="text-[13px] font-extrabold text-ink-strong">{entry.createdAt}</span>
                  <strong className="text-xs text-accent-strong">{formatHistorySource(entry.source)}</strong>
                  <em className="text-xs not-italic text-ink-muted">
                    {entry.lineCount} 行 · {formatBytes(entry.byteSize)}
                  </em>
                </button>
              ))}
            </aside>

            <section className="flex min-h-0 min-w-0 flex-col bg-surface" aria-label="恢复预览">
              {errorMessage && (
                <p className="m-3 rounded-[7px] border border-[rgba(var(--danger-rgb),0.26)] bg-danger-soft px-2.5 py-[9px] text-[13px] leading-normal text-danger">
                  {errorMessage}
                </p>
              )}
              {isDirty && (
                <p className="m-3 rounded-[7px] border border-[rgba(var(--warning-rgb),0.28)] bg-warning-soft px-2.5 py-[9px] text-[13px] leading-normal text-warning">
                  请先保存当前草稿后再恢复版本。
                </p>
              )}
              {selectedDetail && diff ? (
                <>
                  <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border px-3 py-[9px] text-xs font-extrabold text-ink-muted">
                    <span className="inline-flex min-w-0 items-center gap-1.5">
                      <History size={14} />
                      {selectedDetail.createdAt}
                    </span>
                    <span className="inline-flex min-w-0 items-center gap-1.5">
                      +{diff.stats.addedLines} / -{diff.stats.removedLines}
                    </span>
                  </div>
                  <div className="min-h-0 overflow-auto">
                    <div className={unifiedDiffFileClassName}>
                      <span className="min-w-0 truncate">{"当前内容 -> 历史版本"}</span>
                      <em className="not-italic">{formatHistorySource(selectedDetail.source)}</em>
                    </div>
                    {diff.hunks.map((hunk) => (
                      <div className="border-b border-[#eef2f4]" key={hunk.id}>
                        <div className={diffHunkHeaderClassName}>
                          <span>
                            @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
                          </span>
                          {(hunk.hiddenBefore > 0 || hunk.hiddenAfter > 0) && (
                            <em className="ml-auto font-inherit not-italic text-ink-muted">折叠 {hunk.hiddenBefore + hunk.hiddenAfter} 行</em>
                          )}
                        </div>
                        <div className="flex flex-col">
                          {hunk.lines.map((line) => (
                            <div className={cn(diffLineGridClassName, diffLineToneClassName(line.kind))} key={line.id}>
                              <span className={cn(diffGutterClassName, "diff-line-number-old")}>{line.originalLineNumber ?? ""}</span>
                              <span className={cn(diffGutterClassName, "diff-line-number-new")}>{line.nextLineNumber ?? ""}</span>
                              <span className={cn(diffGutterClassName, "diff-line-marker justify-center px-0")}>{getDiffLineMarker(line.kind)}</span>
                              <code className="min-w-0 px-2 py-0.5 whitespace-pre-wrap [overflow-wrap:anywhere]">{line.text || " "}</code>
                            </div>
                          ))}
                        </div>
                      </div>
                    ))}
                  </div>
                </>
              ) : (
                !isLoading && <p className="m-3 text-[13px] leading-normal text-ink-muted">选择一个版本查看差异</p>
              )}
            </section>
          </div>

          <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-border px-4 py-3.5 max-[760px]:flex-col-reverse max-[760px]:items-stretch">
            <Button variant="ghost" onClick={requestClear} disabled={isActionDisabled || !entries.length}>
              <Trash2 size={15} />
              清空当前文件历史
            </Button>
            <Button
              variant="primary"
              size="compact"
              onClick={requestRestore}
              disabled={isActionDisabled || isDirty || !selectedDetail}
            >
              <RotateCcw size={15} />
              恢复此版本
            </Button>
          </footer>
        </ModalPanel>
      </ModalBackdrop>
      {confirmation && (
        <ConfirmDialog
          {...confirmation}
          isBusy={isWorking}
          onCancel={() => setConfirmation(null)}
          onConfirm={() => void confirmation.onConfirm()}
        />
      )}
    </>
  );
}

/** 把未知异常转换为弹窗短错误，避免渲染空对象。 */
function formatDialogError(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 历史来源短标签，和后端 DocumentHistorySource 保持同名映射。 */
function formatHistorySource(source: DocumentHistorySource) {
  if (source === "agent-change") {
    return "Agent 写入";
  }

  if (source === "restore") {
    return "回档前";
  }

  return "手动保存";
}

/** 字节数使用短格式展示，避免版本列表横向拥挤。 */
function formatBytes(byteSize: number) {
  if (byteSize < 1024) {
    return `${byteSize} B`;
  }

  return `${(byteSize / 1024).toFixed(byteSize < 1024 * 1024 ? 1 : 0)} KB`;
}

/** 根据 diff 行类型选择已有审阅样式。 */
/** 根据 diff 行类型生成 unified diff 常见标记。 */
function getDiffLineMarker(kind: "context" | "added" | "removed" | "placeholder") {
  if (kind === "added") {
    return "+";
  }

  if (kind === "removed") {
    return "-";
  }

  return " ";
}
