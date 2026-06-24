import { useCallback, useEffect, useRef } from "react";
import type { UIEventHandler } from "react";

/** 分屏滚动同步来源，用于区分用户当前滚动的是源码还是预览。 */
type MarkdownScrollPane = "editor" | "preview";

/** 根据滚动容器当前状态计算相对进度，内容不足一屏时固定为顶部。 */
function getScrollRatio(element: HTMLElement) {
  const maxScrollTop = element.scrollHeight - element.clientHeight;

  if (maxScrollTop <= 0) {
    return 0;
  }

  return element.scrollTop / maxScrollTop;
}

/** 将目标容器滚动到指定相对进度，并限制在浏览器可接受的滚动范围内。 */
function applyScrollRatio(element: HTMLElement, ratio: number) {
  const maxScrollTop = element.scrollHeight - element.clientHeight;
  const nextScrollTop = Math.min(Math.max(maxScrollTop * ratio, 0), Math.max(maxScrollTop, 0));

  element.scrollTop = nextScrollTop;
}

/** 管理 Markdown 分屏模式下源码 textarea 和预览区的双向比例滚动同步。 */
export function useSyncedMarkdownScroll(isEnabled: boolean) {
  /** Markdown 源码编辑器滚动容器，textarea 自身承担滚动。 */
  const editorRef = useRef<HTMLTextAreaElement | null>(null);
  /** Markdown 预览滚动容器，渲染后的 HTML 内容在这个 div 内滚动。 */
  const previewRef = useRef<HTMLDivElement | null>(null);
  /** 被程序化滚动的目标侧；其随后触发的 scroll 事件需要忽略一次，避免循环同步。 */
  const ignoredPaneRef = useRef<MarkdownScrollPane | null>(null);
  /** 清理忽略标记的动画帧，目标侧没有触发 scroll 事件时也能恢复正常监听。 */
  const clearIgnoredPaneFrameRef = useRef<number | null>(null);

  /** 取消待执行的动画帧，避免快速连续滚动时旧清理任务抢先重置状态。 */
  const cancelPendingClear = useCallback(() => {
    if (clearIgnoredPaneFrameRef.current !== null) {
      window.cancelAnimationFrame(clearIgnoredPaneFrameRef.current);
      clearIgnoredPaneFrameRef.current = null;
    }
  }, []);

  /** 延后一帧清理忽略标记，覆盖目标侧没有实际产生 scroll 事件的情况。 */
  const scheduleIgnoredPaneClear = useCallback(
    (pane: MarkdownScrollPane) => {
      cancelPendingClear();
      clearIgnoredPaneFrameRef.current = window.requestAnimationFrame(() => {
        if (ignoredPaneRef.current === pane) {
          ignoredPaneRef.current = null;
        }

        clearIgnoredPaneFrameRef.current = null;
      });
    },
    [cancelPendingClear],
  );

  /** 从用户滚动的一侧读取相对位置，并把另一侧滚动到对应进度。 */
  const syncScrollFromPane = useCallback(
    (pane: MarkdownScrollPane) => {
      if (!isEnabled) {
        return;
      }

      if (ignoredPaneRef.current === pane) {
        ignoredPaneRef.current = null;
        cancelPendingClear();
        return;
      }

      const sourceElement = pane === "editor" ? editorRef.current : previewRef.current;
      const targetElement = pane === "editor" ? previewRef.current : editorRef.current;
      const targetPane = pane === "editor" ? "preview" : "editor";

      if (!sourceElement || !targetElement) {
        return;
      }

      // 使用相对滚动比例同步，避免 Markdown 渲染后块高度变化导致逐行映射失准。
      ignoredPaneRef.current = targetPane;
      applyScrollRatio(targetElement, getScrollRatio(sourceElement));
      scheduleIgnoredPaneClear(targetPane);
    },
    [cancelPendingClear, isEnabled, scheduleIgnoredPaneClear],
  );

  /** 编辑器滚动事件处理器，只在 split 模式启用时推动预览区跟随。 */
  const handleEditorScroll: UIEventHandler<HTMLTextAreaElement> = useCallback(() => {
    syncScrollFromPane("editor");
  }, [syncScrollFromPane]);

  /** 预览区滚动事件处理器，只在 split 模式启用时推动源码区跟随。 */
  const handlePreviewScroll: UIEventHandler<HTMLDivElement> = useCallback(() => {
    syncScrollFromPane("preview");
  }, [syncScrollFromPane]);

  useEffect(() => {
    if (isEnabled) {
      return undefined;
    }

    ignoredPaneRef.current = null;
    cancelPendingClear();

    return undefined;
  }, [cancelPendingClear, isEnabled]);

  useEffect(() => {
    return () => cancelPendingClear();
  }, [cancelPendingClear]);

  return {
    editorRef,
    previewRef,
    handleEditorScroll,
    handlePreviewScroll,
  };
}
