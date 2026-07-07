import { useEffect, useState } from "react";
import { loadDocumentPreview } from "../shared/tauriApi";
import type { DocumentPreview, WorkspaceSnapshot } from "../shared/types";

/** 将未知异常统一转换为可展示文案，避免文档预览错误区域渲染空对象。 */
function formatPreviewErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** 根据当前激活文档加载只读预览，TXT 文档继续走正文编辑器不触发预览请求。 */
export function useDocumentPreview(snapshot: WorkspaceSnapshot | null) {
  /** 非 TXT 文档的只读预览数据，由 Tauri 后端按类型生成。 */
  const [documentPreview, setDocumentPreview] = useState<DocumentPreview | null>(null);
  /** 文档预览加载失败文案，直接展示给用户且不包含路径或二进制内容。 */
  const [documentPreviewError, setDocumentPreviewError] = useState("");
  /** 文档预览加载状态，用于右侧面板展示轻量 loading。 */
  const [isDocumentPreviewLoading, setIsDocumentPreviewLoading] = useState(false);

  useEffect(() => {
    if (!snapshot?.activeDocumentId) {
      setDocumentPreview(null);
      setDocumentPreviewError("");
      setIsDocumentPreviewLoading(false);
      return;
    }

    const activeDocument = snapshot.documents.find((document) => document.id === snapshot.activeDocumentId);

    if (!activeDocument || activeDocument.fileType === "txt") {
      setDocumentPreview(null);
      setDocumentPreviewError("");
      setIsDocumentPreviewLoading(false);
      return;
    }

    let isMounted = true;

    setDocumentPreview(null);
    setDocumentPreviewError("");
    setIsDocumentPreviewLoading(true);

    void loadDocumentPreview(snapshot, activeDocument.id)
      .then((preview) => {
        if (isMounted) {
          setDocumentPreview(preview);
        }
      })
      .catch((error) => {
        if (isMounted) {
          setDocumentPreviewError(formatPreviewErrorMessage(error));
        }
      })
      .finally(() => {
        if (isMounted) {
          setIsDocumentPreviewLoading(false);
        }
      });

    return () => {
      isMounted = false;
    };
  }, [snapshot?.activeDocumentId, snapshot?.documents]);

  return {
    documentPreview,
    documentPreviewError,
    isDocumentPreviewLoading,
  };
}
