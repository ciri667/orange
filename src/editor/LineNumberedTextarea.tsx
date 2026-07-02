import { forwardRef, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  ChangeEventHandler,
  ClipboardEventHandler,
  CSSProperties,
  KeyboardEventHandler,
  Ref,
  UIEventHandler,
} from "react";
import { logDebug, logWarn } from "../shared/logger";
import { splitLogicalLines } from "./lineNumberUtils";

/** 行号编辑器支持的源码文件类型；只记录类型，不记录文件名、路径或正文。 */
type LineNumberedTextareaFileType = "markdown" | "txt";

/** 行号编辑器入参，保留 textarea 的核心编辑事件并补充脱敏日志上下文。 */
interface LineNumberedTextareaProps {
  value: string;
  className?: string;
  fileType: LineNumberedTextareaFileType;
  onChange: ChangeEventHandler<HTMLTextAreaElement>;
  onPaste?: ClipboardEventHandler<HTMLTextAreaElement>;
  onKeyDown?: KeyboardEventHandler<HTMLTextAreaElement>;
  onScroll?: UIEventHandler<HTMLTextAreaElement>;
  spellCheck?: boolean;
  ariaLabel: string;
}

/** 默认行高兜底值，只有浏览器无法提供 textarea 计算样式时使用。 */
const FALLBACK_LINE_HEIGHT = 24;

/** 将 textarea ref 同时写入内部状态和外部调用方 ref，确保现有滚动同步逻辑仍能访问原生控件。 */
function assignTextareaRef(ref: Ref<HTMLTextAreaElement>, element: HTMLTextAreaElement | null) {
  if (typeof ref === "function") {
    ref(element);
    return;
  }

  if (ref) {
    ref.current = element;
  }
}

/** 判断两组行高是否有可见差异，避免每次测量都触发无意义重渲染。 */
function areLineHeightsEqual(currentHeights: number[], nextHeights: number[]) {
  if (currentHeights.length !== nextHeights.length) {
    return false;
  }

  return currentHeights.every((height, index) => Math.abs(height - nextHeights[index]) < 0.5);
}

/** 解析 textarea 的计算行高，line-height 为 normal 时回退到字体大小的常见比例。 */
function parseLineHeight(style: CSSStyleDeclaration) {
  const computedLineHeight = Number.parseFloat(style.lineHeight);

  if (Number.isFinite(computedLineHeight) && computedLineHeight > 0) {
    return computedLineHeight;
  }

  const fontSize = Number.parseFloat(style.fontSize);

  if (Number.isFinite(fontSize) && fontSize > 0) {
    return fontSize * 1.72;
  }

  return FALLBACK_LINE_HEIGHT;
}

/** 将行数收敛成区间，避免精确文档长度进入诊断日志。 */
function getLineCountBucket(lineCount: number) {
  if (lineCount < 100) {
    return "1-99";
  }

  if (lineCount < 1000) {
    return "100-999";
  }

  if (lineCount < 10000) {
    return "1000-9999";
  }

  return "10000+";
}

/** 带真实行号的 textarea，使用隐藏测量层保持软换行场景下的行号垂直对齐。 */
export const LineNumberedTextarea = forwardRef<HTMLTextAreaElement, LineNumberedTextareaProps>(function LineNumberedTextarea(
  {
    value,
    className = "",
    fileType,
    onChange,
    onPaste,
    onKeyDown,
    onScroll,
    spellCheck = false,
    ariaLabel,
  },
  forwardedRef,
) {
  /** 原生 textarea 仍是唯一输入控件，业务事件和滚动同步都绑定在它身上。 */
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  /** 行号滚动层只展示编号，不接收指针事件，也不会进入辅助技术阅读顺序。 */
  const gutterScrollRef = useRef<HTMLDivElement | null>(null);
  /** 隐藏测量层复刻 textarea 文本排版，用于计算每个真实行在软换行后的视觉高度。 */
  const measureRef = useRef<HTMLDivElement | null>(null);
  /** 文本内容区域宽度会受行号位数影响，宽度变化后需要重新测量软换行高度。 */
  const [contentWidth, setContentWidth] = useState(0);
  /** 当前排版行高，测量失败时用它作为每个真实行的最小高度。 */
  const [lineHeight, setLineHeight] = useState(FALLBACK_LINE_HEIGHT);
  /** 每个真实行占据的视觉高度，驱动 gutter 中每个编号块的高度。 */
  const [lineHeights, setLineHeights] = useState<number[]>([FALLBACK_LINE_HEIGHT]);
  /** 测量降级只写一次 warn，避免持续输入时刷屏。 */
  const hasLoggedMeasurementWarningRef = useRef(false);

  const logicalLines = useMemo(() => splitLogicalLines(value), [value]);
  const digitCount = String(logicalLines.length).length;
  const wrapperClassName = ["line-numbered-textarea", className].filter(Boolean).join(" ");
  const textareaClassName = ["markdown-editor", "line-numbered-textarea-control", className].filter(Boolean).join(" ");
  /** 行号 gutter 的宽度 token，随行数位数变化但不触发布局跳变。 */
  const gutterWidthToken = `max(48px, calc(${digitCount}ch + 28px))`;
  const wrapperStyle = { "--line-number-gutter-width": gutterWidthToken } as CSSProperties;

  /** 更新文本内容区域尺寸；ResizeObserver 和内容位数变化都会走这里。 */
  const updateTextMetrics = useCallback(() => {
    const textarea = textareaRef.current;

    if (!textarea) {
      return;
    }

    const style = window.getComputedStyle(textarea);
    const paddingLeft = Number.parseFloat(style.paddingLeft) || 0;
    const paddingRight = Number.parseFloat(style.paddingRight) || 0;
    const nextContentWidth = Math.max(textarea.clientWidth - paddingLeft - paddingRight, 0);
    const nextLineHeight = parseLineHeight(style);

    setContentWidth((currentWidth) => (Math.abs(currentWidth - nextContentWidth) < 0.5 ? currentWidth : nextContentWidth));
    setLineHeight((currentLineHeight) =>
      Math.abs(currentLineHeight - nextLineHeight) < 0.5 ? currentLineHeight : nextLineHeight,
    );
  }, []);

  /** callback ref 能让外部 hook 和内部测量逻辑同时拿到同一个 textarea 节点。 */
  const setTextareaElement = useCallback(
    (element: HTMLTextAreaElement | null) => {
      textareaRef.current = element;
      assignTextareaRef(forwardedRef, element);

      if (element) {
        updateTextMetrics();
      }
    },
    [forwardedRef, updateTextMetrics],
  );

  /** textarea 滚动时同步 gutter 位置，再交给调用方处理 Markdown 分屏滚动。 */
  const handleTextareaScroll: UIEventHandler<HTMLTextAreaElement> = useCallback(
    (event) => {
      if (gutterScrollRef.current) {
        // gutter 内容按 textarea scrollTop 反向平移，保证编号和正文始终共用同一滚动位置。
        gutterScrollRef.current.style.transform = `translateY(-${event.currentTarget.scrollTop}px)`;
      }

      onScroll?.(event);
    },
    [onScroll],
  );

  useEffect(() => {
    logDebug("行号编辑器已挂载。", {
      category: "editor",
      event: "line_numbered_textarea_mount",
      status: "ready",
      metadata: {
        fileType,
        lineCountBucket: getLineCountBucket(logicalLines.length),
        softWrap: true,
      },
    });
  }, [fileType]);

  useEffect(() => {
    const textarea = textareaRef.current;

    if (!textarea) {
      return undefined;
    }

    updateTextMetrics();

    const resizeObserver = new ResizeObserver(() => updateTextMetrics());
    resizeObserver.observe(textarea);

    return () => resizeObserver.disconnect();
  }, [updateTextMetrics]);

  useLayoutEffect(() => {
    updateTextMetrics();
  }, [digitCount, updateTextMetrics]);

  useLayoutEffect(() => {
    const measureElement = measureRef.current;

    if (!measureElement || contentWidth <= 0) {
      const fallbackHeights = logicalLines.map(() => lineHeight);
      setLineHeights((currentHeights) => (areLineHeightsEqual(currentHeights, fallbackHeights) ? currentHeights : fallbackHeights));

      if (!hasLoggedMeasurementWarningRef.current && contentWidth <= 0 && textareaRef.current?.clientWidth === 0) {
        hasLoggedMeasurementWarningRef.current = true;
        logWarn("行号编辑器测量宽度不可用，已使用默认行高。", {
          category: "editor",
          event: "line_numbered_textarea_measure",
          status: "fallback",
          metadata: {
            fileType,
            reason: "empty_content_width",
            lineCountBucket: getLineCountBucket(logicalLines.length),
          },
        });
      }

      return;
    }

    const measuredHeights = Array.from(measureElement.children).map((lineElement) => {
      const measuredHeight = lineElement.getBoundingClientRect().height;

      return Math.max(measuredHeight, lineHeight);
    });

    setLineHeights((currentHeights) => (areLineHeightsEqual(currentHeights, measuredHeights) ? currentHeights : measuredHeights));
  }, [contentWidth, fileType, lineHeight, logicalLines]);

  return (
    <div className={wrapperClassName} style={wrapperStyle} data-file-type={fileType} data-gutter-width={gutterWidthToken}>
      <div className="line-number-gutter" aria-hidden="true">
        <div className="line-number-gutter-scroll" ref={gutterScrollRef}>
          {logicalLines.map((_line, index) => (
            <div className="line-number-row" key={index} style={{ height: lineHeights[index] ?? lineHeight }}>
              {index + 1}
            </div>
          ))}
        </div>
      </div>
      <textarea
        className={textareaClassName}
        ref={setTextareaElement}
        value={value}
        onChange={onChange}
        onPaste={onPaste}
        onKeyDown={onKeyDown}
        onScroll={handleTextareaScroll}
        spellCheck={spellCheck}
        aria-label={ariaLabel}
      />
      <div
        className="line-number-measure"
        ref={measureRef}
        style={{ width: contentWidth || undefined }}
        aria-hidden="true"
      >
        {logicalLines.map((line, index) => (
          <div className="line-number-measure-row" key={index}>
            {line || "\u00a0"}
          </div>
        ))}
      </div>
    </div>
  );
});
