import { convertFileSrc } from "@tauri-apps/api/core";
import { Clock3, Eye, FilePenLine, FileText, Save, Trash2 } from "lucide-react";
import type { DocumentPreview, KnowledgeBase, WorkspaceDocument } from "../shared/types";

/** 格式化纯文本文档的阅读统计，用于保持 txt 编辑体验与 Markdown 面板一致。 */
function getTextStats(content: string) {
  const words = content.replace(/\s+/g, "").length;
  const lines = content ? content.split(/\r?\n/).length : 0;

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

/** 普通文档面板，txt 可编辑，docx/pdf 只读预览。 */
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
  onRenameDocument: () => void;
  onDeleteDocument: () => void;
}) {
  if (!document) {
    return (
      <section className="editor-pane" aria-label="文档预览">
        <header className="editor-header">
          <div>
            <p className="path-label">{knowledgeBase.name}</p>
            <h2>暂无文档</h2>
          </div>
          <div className="editor-actions" />
        </header>
        <div className="editor-empty-state">
          <strong>当前知识库还没有支持文档。</strong>
          <span>请在左侧目录树中新建 Markdown 或 TXT，或在本地目录中添加支持文件后重新扫描。</span>
        </div>
      </section>
    );
  }

  const content = document.content ?? "";
  const stats = getTextStats(content);
  const isTextDocument = document.fileType === "txt";

  return (
    <section className="editor-pane" aria-label="普通文档">
      <header className="editor-header">
        <div>
          <p className="path-label">
            {knowledgeBase.name} / {document.path}
          </p>
          <h2>{document.title}</h2>
        </div>
        <div className="editor-actions">
          {isTextDocument && (
            <>
              <button className="text-button" type="button" onClick={onSaveDocument} disabled={isBusy || !isDirty}>
                <Save size={16} />
                {isDirty ? "保存草稿" : "已保存"}
              </button>
              <button className="text-button" type="button" title="重命名当前 TXT" onClick={onRenameDocument} disabled={isBusy}>
                <FilePenLine size={18} />
                重命名
              </button>
              <button className="text-button danger" type="button" title="删除当前 TXT" onClick={onDeleteDocument} disabled={isBusy}>
                <Trash2 size={18} />
                删除
              </button>
            </>
          )}
        </div>
      </header>

      <div className="meta-strip">
        <span>
          <Clock3 size={14} />
          {document.updatedAt}
        </span>
        <span>
          <FileText size={14} />
          {getDocumentTypeLabel(document)}
        </span>
        {isTextDocument ? (
          <>
            <span>
              <FilePenLine size={14} />
              {stats.words} 字，{stats.lines} 行
            </span>
            <span className={isDirty ? "dirty-indicator" : ""}>
              <Save size={14} />
              {isDirty ? "未保存草稿" : "已保存到本地"}
            </span>
          </>
        ) : (
          <span>
            <Eye size={14} />
            只读预览
          </span>
        )}
      </div>

      {isTextDocument ? (
        <textarea
          className="markdown-editor plain-text-editor"
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
          aria-label="当前 TXT 文档内容"
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

/** 只读文档预览区域，按 docx/pdf 分支展示。 */
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
    return <div className="document-preview-state">正在加载预览...</div>;
  }

  if (previewError) {
    return <div className="document-preview-state error">{previewError}</div>;
  }

  if (document.fileType === "pdf") {
    const assetUrl = preview?.assetPath && isTauriAssetRuntime() ? convertFileSrc(preview.assetPath) : "";

    return assetUrl ? (
      <iframe className="document-pdf-preview" title={document.title} src={assetUrl} />
    ) : (
      <div className="document-preview-state">当前环境无法内嵌 PDF 预览。</div>
    );
  }

  if (document.fileType === "docx") {
    const blocks = preview?.blocks ?? [];

    return (
      <div className="document-docx-preview" aria-label="DOCX 预览">
        {blocks.map((block, index) =>
          block.type === "heading" ? (
            <h3 key={`${block.type}-${index}`}>{block.text}</h3>
          ) : (
            <p key={`${block.type}-${index}`}>{block.text}</p>
          ),
        )}
      </div>
    );
  }

  return <div className="document-preview-state">该文档类型暂不支持预览。</div>;
}

/** 把文档类型转换成界面短标签。 */
function getDocumentTypeLabel(document: WorkspaceDocument) {
  if (document.fileType === "txt") {
    return "TXT 文档";
  }

  if (document.fileType === "docx") {
    return "DOCX 文档";
  }

  return "PDF 文档";
}
