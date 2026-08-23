import { convertFileSrc } from "@tauri-apps/api/core";
import { Clock3, Eye, FilePenLine, FileText, Save } from "lucide-react";
import { Button } from "../shared/Button";
import { logInfo, logWarn } from "../shared/logger";
import type { DocumentFileType, DocumentPreview, ExportFormat, KnowledgeBase, WorkspaceDocument } from "../shared/types";
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
}) {
  if (!document) {
    return (
      <section className="editor-pane" aria-label="文档预览">
        <EditorEmptyHeader pathLabel={knowledgeBase.name} pathLogArea="document_empty_knowledge_base" title="暂无文档" />
        <div className="grid min-h-0 place-content-center justify-items-center gap-2.5 rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-ink-muted">
          <strong className="text-base text-ink">当前知识库还没有支持文档。</strong>
          <span className="max-w-[420px] text-[13px] leading-[1.55]">
            请在左侧目录树中新建 Markdown 或 TXT，或在本地目录中添加支持文件后重新扫描。
          </span>
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
            <Button variant="text" onClick={onSaveDocument} disabled={isBusy || !isDirty}>
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

      <EditorMetaStrip
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

      {isTextDocument ? (
        <LineNumberedTextarea
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
        />
      )}
    </section>
  );
}

/** 只读文档预览区域，按 docx/pdf/图片分支展示。 */
function DocumentPreviewView({
  document,
  preview,
  previewError,
  isPreviewLoading,
}: {
  document: WorkspaceDocument;
  preview?: DocumentPreview;
  previewError: string;
  isPreviewLoading: boolean;
}) {
  if (isPreviewLoading) {
    return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">正在加载预览...</div>;
  }

  if (previewError) {
    return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-[rgba(var(--danger-rgb),0.28)] bg-danger-soft p-6 text-center text-[13px] leading-[1.55] text-danger">{previewError}</div>;
  }

  if (document.fileType === "pdf") {
    const assetUrl = createDocumentAssetUrl(preview?.assetPath);

    return (
      <div className="grid min-h-0 gap-3" aria-label="PDF 预览">
        {assetUrl ? (
          <iframe className="h-full min-h-0 w-full min-w-0 overflow-hidden rounded-panel border border-[rgba(230,224,214,0.92)] bg-surface" title={document.title} src={assetUrl} />
        ) : (
          <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">当前环境无法内嵌 PDF 预览。</div>
        )}
        {!!preview?.blocks?.length && (
          <details className="max-h-[280px] overflow-auto rounded-lg border border-border bg-surface-muted p-3">
            <summary className="cursor-pointer font-semibold">查看可提取文本（{preview.blocks.length} 页）</summary>
            {preview.blocks.map((block, index) => (
              <section className="mt-3" key={`pdf-${index}`}>
                <strong>{block.page ? `第 ${block.page} 页` : `片段 ${index + 1}`}</strong>
                <p className="whitespace-pre-wrap">{block.text}</p>
              </section>
            ))}
          </details>
        )}
      </div>
    );
  }

  if (document.fileType === "image") {
    const assetUrl = createDocumentAssetUrl(preview?.assetPath);

    return assetUrl ? (
      <div className="grid min-h-0 min-w-0 place-items-center overflow-auto rounded-panel border border-[rgba(230,224,214,0.92)] bg-surface p-[22px]" aria-label="图片预览">
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
      <div className="min-h-0 min-w-0 overflow-auto rounded-panel border border-[rgba(230,224,214,0.92)] bg-surface p-[22px] text-sm leading-[1.76] text-ink [scrollbar-gutter:stable] [&>:last-child]:mb-0" aria-label="DOCX 预览">
        {blocks.map((block, index) =>
          block.type === "heading" ? (
            <h3 className="mb-3 mt-0 text-[19px] leading-[1.35] text-ink-strong" key={`${block.type}-${index}`}>{block.text}</h3>
          ) : block.type === "table" ? (
            <pre className="whitespace-pre-wrap font-[inherit]" key={`${block.type}-${index}`}>{block.text}</pre>
          ) : (
            <p className="mb-3 mt-0 whitespace-pre-wrap" key={`${block.type}-${index}`}>{block.text}</p>
          ),
        )}
      </div>
    );
  }

  return <div className="grid min-h-0 min-w-0 place-items-center rounded-panel border border-dashed border-border-strong bg-surface p-6 text-center text-[13px] leading-[1.55] text-ink-muted">该文档类型暂不支持预览。</div>;
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
