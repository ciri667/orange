import { convertFileSrc } from "@tauri-apps/api/core";
import { Clock3, Eye, FilePenLine, FileText, Save } from "lucide-react";
import { createElement, useEffect, useRef } from "react";
import { Button } from "../shared/Button";
import { VoiceDictationButton } from "../shared/VoiceDictationButton";
import { logInfo, logWarn } from "../shared/logger";
import type { AgentSession, DocumentFileType, DocumentPreview, ExportFormat, KnowledgeBase, WorkspaceDocument } from "../shared/types";
import { RelatedSessionsButton } from "../agent/RelatedSessionsButton";
import { EditorEmptyHeader, EditorFileHeader, EditorMetaStrip, EditorMoreActionMenu } from "./EditorFileChrome";
import { LineNumberedTextarea } from "./LineNumberedTextarea";
import { countLogicalLines } from "./lineNumberUtils";

/** 单个文档类型对应的导出菜单项，确保 PDF 和图片不展示不支持的转换。 */
const DOCUMENT_EXPORT_OPTIONS: Record<DocumentFileType, Array<{ format: ExportFormat; label: string }>> = {
  txt: [
    { format: "original", label: "原文件 .txt" },
    { format: "markdown", label: "转为 .md" },
    { format: "pdf", label: "转为 .pdf" },
  ],
  docx: [
    { format: "original", label: "原文件 .docx" },
    { format: "markdown", label: "转为 .md" },
    { format: "pdf", label: "转为 .pdf" },
  ],
  pdf: [
    { format: "original", label: "原文件 .pdf" },
    { format: "pdf", label: "转为 .pdf" },
  ],
  image: [{ format: "original", label: "原图片文件" }],
};

/** 格式化纯文本文档的阅读统计，用于保持 txt 编辑体验与 Markdown 面板一致。 */
function getTextStats(content: string) {
  const words = content.replace(/\s+/g, "").length;
  const lines = countLogicalLines(content);

  return { words, lines };
}

/** 判断当前是否具备 Tauri asset 协议转换能力。 */
function isTauriAssetRuntime() {
  if (typeof window === "undefined") {
    return false;
  }

  const tauriInternals = window.__TAURI_INTERNALS__;

  return typeof tauriInternals === "object" && tauriInternals !== null && "convertFileSrc" in tauriInternals;
}

/** 把预览返回的 assetPath 转成可渲染 URL；浏览器模拟态允许 data/blob/http 直通。 */
function createDocumentAssetUrl(assetPath?: string) {
  if (!assetPath) {
    return "";
  }

  if (/^(data:|blob:|https?:)/i.test(assetPath)) {
    return assetPath;
  }

  return isTauriAssetRuntime() ? convertFileSrc(assetPath) : "";
}

/** 普通文档面板，txt 可编辑，docx/pdf/图片只读预览。 */
export function DocumentPane({
  document,
  knowledgeBase,
  preview,
  previewError,
  isPreviewLoading,
  isBusy,
  isDirty,
  onSaveDocument,
  onContentChange,
  onExportFile,
  onOpenHistory,
  onRenameDocument,
  onDeleteDocument,
  onNotice,
  sessions = [],
  onSelectSession,
  focus,
}: {
  document?: WorkspaceDocument;
  knowledgeBase: KnowledgeBase;
  preview?: DocumentPreview;
  previewError: string;
  isPreviewLoading: boolean;
  isBusy: boolean;
  isDirty: boolean;
  onSaveDocument: () => void;
  onContentChange: (content: string) => void;
  onExportFile: (format: ExportFormat) => void | Promise<void>;
  onOpenHistory: () => void;
  onRenameDocument: () => void;
  onDeleteDocument: () => void;
  onNotice?: (message: string) => void;
  sessions?: AgentSession[];
  onSelectSession?: (sessionId: string) => void;
  /** 点击 Agent 引用后要定位的页码/结构块；切换引用时滚动并高亮。 */
  focus?: { page?: number; blockIndex?: number };
}) {
  const textAreaRef = useRef<HTMLTextAreaElement | null>(null);

  if (!document) {
    // 与 Markdown 面板保持一致：区分「知识库没有文档」和「没打开任何文档」，
    // 后者不能断言知识库里没有内容。
    const hasDocuments = knowledgeBase.documentCount > 0;

    return (
      <section className="editor-pane is-empty" aria-label="文档预览">
        <EditorEmptyHeader
          pathLabel={knowledgeBase.name}
          pathLogArea="document_empty_knowledge_base"
          title={hasDocuments ? "未打开文件" : "暂无文档"}
        />
        <div className="grid min-h-0 place-content-center justify-items-center gap-2.5 rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-ink-muted">
          {hasDocuments ? (
            <>
              <strong className="text-base text-ink">编辑区没有打开任何文件。</strong>
              <span className="max-w-[420px] text-[13px] leading-[1.55]">
                从左侧目录树中选择文件即可打开预览。
              </span>
            </>
          ) : (
            <>
              <strong className="text-base text-ink">当前知识库还没有支持文档。</strong>
              <span className="max-w-[420px] text-[13px] leading-[1.55]">
                请在左侧目录树中新建 Markdown 或 TXT，或在本地目录中添加支持文件后重新扫描。
              </span>
            </>
          )}
        </div>
      </section>
    );
  }

  const content = document.content ?? "";
  const stats = getTextStats(content);
  const isTextDocument = document.fileType === "txt";
  const exportOptions = DOCUMENT_EXPORT_OPTIONS[document.fileType];

  return (
    <section className="editor-pane" aria-label="普通文档">
      <EditorFileHeader
        title={{
          pathLabel: `${knowledgeBase.name} / ${document.path}`,
          pathLogArea: "document_path",
          title: document.title,
          titleLogArea: "document_title",
        }}
        actions={
          <>
            {isTextDocument && (
              <Button variant="text" className="editor-save-action" onClick={onSaveDocument} disabled={isBusy || !isDirty}>
                <Save size={16} />
                {isDirty ? "保存草稿" : "已保存"}
              </Button>
            )}
            <EditorMoreActionMenu
              exportOptions={exportOptions}
              isBusy={isBusy}
              logContext={{
                event: "document_more_menu_toggle",
                metadata: { fileType: document.fileType, isBusy, isDirty },
              }}
              onExportFile={onExportFile}
              onOpenHistory={isTextDocument ? onOpenHistory : undefined}
              onRename={isTextDocument ? onRenameDocument : undefined}
              onDelete={isTextDocument ? onDeleteDocument : undefined}
            />
          </>
        }
      />

      <div className="editor-content-column">
        <EditorMetaStrip
          actions={
            <>
              {onSelectSession && <RelatedSessionsButton sessions={sessions} file={{ id: document.id, knowledgeBaseId: document.knowledgeBaseId, path: document.path }} onSelectSession={onSelectSession} />}
              {isTextDocument && <VoiceDictationButton getTextarea={() => textAreaRef.current} onValueChange={onContentChange} onNotice={onNotice} disabled={isBusy} />}
            </>
          }
          items={[
            { icon: <Clock3 size={14} />, text: document.updatedAt },
            { icon: <FileText size={14} />, text: getDocumentTypeLabel(document) },
            ...(isTextDocument
              ? [
                  { icon: <FilePenLine size={14} />, text: `${stats.words} 字，${stats.lines} 行` },
                  {
                    icon: <Save size={14} />,
                    text: isDirty ? "未保存草稿" : "已保存到本地",
                    className: isDirty ? "dirty-indicator" : undefined,
                  },
                ]
              : [{ icon: <Eye size={14} />, text: "只读预览" }]),
          ]}
        />
      </div>

      <div className="editor-body">
        {isTextDocument ? (
          <LineNumberedTextarea
            ref={textAreaRef}
            fileType="txt"
            value={content}
            onChange={(event) => onContentChange(event.target.value)}
            onKeyDown={(event) => {
              // 拦截系统保存快捷键，确保 txt 写入也经过 Tauri hash 和路径校验。
              if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
                event.preventDefault();
                onSaveDocument();
              }
            }}
            spellCheck={false}
            ariaLabel="当前 TXT 文档内容"
          />
        ) : (
          <DocumentPreviewView
            document={document}
            preview={preview}
            previewError={previewError}
            isPreviewLoading={isPreviewLoading}
            focus={focus}
          />
        )}
      </div>
    </section>
  );
}

/** 只读文档预览区域，按 docx/pdf/图片分支展示。 */
function DocumentPreviewView({
  document,
  preview,
  previewError,
  isPreviewLoading,
  focus,
}: {
  document: WorkspaceDocument;
  preview?: DocumentPreview;
  previewError: string;
  isPreviewLoading: boolean;
  focus?: { page?: number; blockIndex?: number };
}) {
  const docxRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (document.fileType !== "docx" || !focus?.blockIndex) {
      return;
    }
    const target = docxRef.current?.querySelector<HTMLElement>(`[data-block-index="${focus.blockIndex}"]`);
    target?.scrollIntoView({ block: "center" });
  }, [document.fileType, focus?.blockIndex, preview?.blocks?.length]);
  if (isPreviewLoading) {
    return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">正在加载预览...</div>;
  }

  if (previewError) {
    return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-[rgba(var(--danger-rgb),0.28)] bg-danger-soft p-6 text-center text-[13px] leading-[1.55] text-danger">{previewError}</div>;
  }

  if (document.fileType === "pdf") {
    const assetUrl = createDocumentAssetUrl(preview?.assetPath);
    const pageCount = preview?.pageCount ?? preview?.blocks?.length ?? 0;
    // PDF 原生阅读器支持 #page= 定位；引用跳转不需要额外渲染引擎。
    const iframeSrc = assetUrl && focus?.page
      ? `${assetUrl}#page=${focus.page}&zoom=page-width`
      : assetUrl;

    return (
      <div className="grid min-h-0 gap-3" aria-label="PDF 预览">
        {!!preview?.warnings?.length && (
          <div className="rounded-lg border border-[rgba(var(--danger-rgb),0.28)] bg-danger-soft px-3 py-2 text-[12px] leading-[1.55] text-danger" role="status">
            {preview.warnings.map((warning) => (
              <p className="m-0" key={warning}>{warning}</p>
            ))}
          </div>
        )}
        {assetUrl ? (
          <iframe className="h-full min-h-0 w-full min-w-0 overflow-hidden rounded-none border-0 bg-surface" title={document.title} src={iframeSrc} />
        ) : (
          <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">当前环境无法内嵌 PDF 预览。</div>
        )}
        {!!preview?.blocks?.length && (
          <details className="max-h-[280px] overflow-auto rounded-lg border border-border bg-surface-muted p-3" open={Boolean(focus?.page)}>
            <summary className="cursor-pointer font-semibold">查看可提取文本（{pageCount} 页）</summary>
            {preview.blocks.map((block, index) => {
              const active = Boolean(focus?.page && block.page === focus.page);
              return (
                <section
                  className={active ? "mt-3 rounded-md bg-surface p-2 ring-2 ring-accent/40" : "mt-3"}
                  data-page={block.page}
                  key={`pdf-${index}`}
                >
                  <strong>{block.page ? `第 ${block.page} 页` : `片段 ${index + 1}`}</strong>
                  <p className="whitespace-pre-wrap">{block.text}</p>
                </section>
              );
            })}
          </details>
        )}
      </div>
    );
  }

  if (document.fileType === "image") {
    const assetUrl = createDocumentAssetUrl(preview?.assetPath);

    return assetUrl ? (
      <div className="grid min-h-0 min-w-0 place-items-center overflow-auto rounded-none border-0 bg-surface p-[22px]" aria-label="图片预览">
        <img
          className="block max-h-full max-w-full object-contain"
          src={assetUrl}
          alt={document.title}
          onLoad={(event) => {
            // 图片加载日志只记录渲染尺寸和文档类型，避免把本地路径写入日志。
            logInfo("图片文档预览加载完成。", {
              category: "frontend",
              event: "document_image_preview",
              status: "loaded",
              metadata: {
                fileType: document.fileType,
                naturalWidth: event.currentTarget.naturalWidth,
                naturalHeight: event.currentTarget.naturalHeight,
              },
            });
          }}
          onError={(event) => {
            logWarn("图片文档预览加载失败。", {
              category: "frontend",
              event: "document_image_preview",
              status: "failed",
              metadata: {
                fileType: document.fileType,
                renderedWidth: event.currentTarget.clientWidth,
                renderedHeight: event.currentTarget.clientHeight,
              },
            });
          }}
        />
      </div>
    ) : (
      <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">当前环境无法内嵌图片预览。</div>
    );
  }

  if (document.fileType === "docx") {
    const blocks = preview?.blocks ?? [];

    return (
      <div className="min-h-0 min-w-0 overflow-auto rounded-none border-0 bg-surface py-4 text-body leading-[1.75] text-ink [scrollbar-gutter:stable] [&>:last-child]:mb-0" aria-label="DOCX 预览" ref={docxRef}>
        {blocks.map((block, index) => {
          const blockIndex = block.index ?? index + 1;
          const active = focus?.blockIndex === blockIndex;
          const commonProps = {
            "data-block-index": blockIndex,
            className: active ? "rounded-md bg-surface-muted px-2 ring-2 ring-accent/40" : undefined,
          } as const;

          return block.type === "heading" ? (
            <DocxHeading key={`${block.type}-${index}`} level={block.headingLevel} text={block.text} {...commonProps} />
          ) : block.type === "table" ? (
            <pre className={`whitespace-pre-wrap font-[inherit] ${commonProps.className ?? ""}`} data-block-index={blockIndex} key={`${block.type}-${index}`}>{block.text}</pre>
          ) : (
            <p className={`mb-3 mt-0 whitespace-pre-wrap ${commonProps.className ?? ""}`} data-block-index={blockIndex} key={`${block.type}-${index}`}>{block.text}</p>
          );
        })}
      </div>
    );
  }

  return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">该文档类型暂不支持预览。</div>;
}

/** 按 DOCX 标题级别渲染，缺失级别时沿用旧版的 h3。 */
function DocxHeading({
  level,
  text,
  className,
  ...rest
}: {
  level?: number;
  text: string;
  className?: string;
  "data-block-index"?: number;
}) {
  const normalizedLevel = Math.min(6, Math.max(1, level ?? 3));
  const tag = `h${normalizedLevel}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
  const sizeClass =
    normalizedLevel === 1
      ? "text-xl"
      : normalizedLevel === 2
        ? "text-lg"
        : normalizedLevel === 3
          ? "text-base"
          : "text-sm";

  return createElement(
    tag,
    { className: `mb-3 mt-0 ${sizeClass} font-semibold leading-[1.35] text-ink-strong ${className ?? ""}`, ...rest },
    text,
  );
}

/** 把文档类型转换成界面短标签。 */
function getDocumentTypeLabel(document: WorkspaceDocument) {
  if (document.fileType === "txt") {
    return "TXT 文档";
  }

  if (document.fileType === "docx") {
    return "DOCX 文档";
  }

  if (document.fileType === "image") {
    return "图片";
  }

  return "PDF 文档";
}
