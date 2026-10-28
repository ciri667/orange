import { renderAsync } from "docx-preview";
import { useEffect, useRef, useState } from "react";

/**
 * 用 docx-preview 渲染 DOCX 的版式预览（表格、图片、页眉页脚、分页）。
 *
 * 只从本地 asset URL 读取一次字节，不加载任何外部资源；渲染失败时把错误
 * 交给父组件决定回退到结构块文本视图，而不是显示空白。
 */
export function DocxLayoutPreview({
  src,
  title,
  onError,
}: {
  src: string;
  title: string;
  onError: (message: string) => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [isRendering, setIsRendering] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setIsRendering(true);

    async function render() {
      try {
        const response = await fetch(src);
        if (!response.ok) {
          throw new Error(`读取文件失败（${response.status}）`);
        }
        const buffer = await response.arrayBuffer();
        if (cancelled || !containerRef.current) {
          return;
        }
        containerRef.current.innerHTML = "";
        await renderAsync(buffer, containerRef.current, undefined, {
          className: "docx-layout",
          inWrapper: true,
          breakPages: true,
          ignoreWidth: false,
          ignoreHeight: true,
          ignoreFonts: false,
          renderHeaders: true,
          renderFooters: true,
          renderFootnotes: true,
          renderEndnotes: true,
          // 图片内联成 data URL，避免生成外部 blob 引用或额外网络请求。
          useBase64URL: true,
        });
        if (!cancelled) {
          setIsRendering(false);
        }
      } catch (error) {
        if (!cancelled) {
          setIsRendering(false);
          onError(error instanceof Error ? error.message : "DOCX 版式渲染失败。");
        }
      }
    }

    void render();
    return () => {
      cancelled = true;
    };
  }, [onError, src]);

  return (
    <div className="min-h-0 min-w-0 overflow-auto rounded-none border-0 bg-surface-muted py-4" aria-label={`${title} 版式预览`}>
      {isRendering ? (
        <div className="px-4 text-[13px] text-ink-muted">正在渲染版式...</div>
      ) : null}
      <div ref={containerRef} className="docx-layout-host mx-auto w-fit max-w-full" />
    </div>
  );
}