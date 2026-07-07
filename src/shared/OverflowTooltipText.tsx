import {
  createElement,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type FocusEvent,
  type HTMLAttributes,
  type MouseEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { logDebug } from "./logger";

type OverflowTooltipElement = "span" | "strong" | "p" | "h2" | "h3" | "small" | "code" | "time" | "em";
type OverflowTooltipTrigger = "hover" | "focus";

interface TooltipPosition {
  left: number;
  maxWidth: number;
  top: number;
}

/** Tooltip 展示原因只按用户可观察行为归类，日志不记录具体文本。 */
type TooltipTriggerSource = OverflowTooltipTrigger;

interface OverflowTooltipTextProps extends Omit<HTMLAttributes<HTMLElement>, "children"> {
  /** 实际渲染的 HTML 标签，默认 span，便于复用现有 CSS 选择器。 */
  as?: OverflowTooltipElement;
  /** 界面上的文本内容；Tooltip 默认展示同一段文本。 */
  text: string;
  /** 少数场景可以用更完整的描述覆盖界面文本，例如摘要拼接后的完整字符串。 */
  tooltipText?: string;
  /** 日志区域标识，只记录位置和长度，不记录文本内容。 */
  logArea: string;
  /** 是否允许非交互文本获得键盘焦点；默认不增加额外 tab stop。 */
  focusable?: boolean;
  /** 渲染内容需要包含额外内联节点时使用；Tooltip 仍以 text/tooltipText 为准。 */
  children?: ReactNode;
  /** time 标签需要透传 dateTime，HTMLAttributes<HTMLElement> 不包含这个字段。 */
  dateTime?: string;
}

/** 判断一个元素的可视文本是否被 CSS 单行省略或固定高度裁剪。 */
function isElementOverflowing(element: HTMLElement) {
  return element.scrollWidth > element.clientWidth + 1 || element.scrollHeight > element.clientHeight + 1;
}

/** 找到承载鼠标和键盘操作的父元素，保证按钮内的省略文本也能响应焦点。 */
function getTooltipInteractionTarget(target: HTMLElement) {
  const interactiveParent = target.closest<HTMLElement>(
    "button, a, input, textarea, select, [role='button'], [role='option'], [tabindex]",
  );

  return interactiveParent ?? target;
}

/** 根据目标元素和 Tooltip 尺寸，把浮层限制在当前视口内。 */
function getTooltipPosition(target: HTMLElement, tooltip: HTMLDivElement | null): TooltipPosition {
  const margin = 12;
  const gap = 8;
  const targetRect = target.getBoundingClientRect();
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const maxWidth = Math.min(520, Math.max(180, viewportWidth - margin * 2));
  const tooltipWidth = tooltip?.getBoundingClientRect().width ?? Math.min(Math.max(targetRect.width, 220), maxWidth);
  const tooltipHeight = tooltip?.getBoundingClientRect().height ?? 0;
  const centeredLeft = targetRect.left + targetRect.width / 2 - tooltipWidth / 2;
  const left = Math.min(Math.max(margin, centeredLeft), Math.max(margin, viewportWidth - tooltipWidth - margin));
  const bottomTop = targetRect.bottom + gap;
  const canFitBelow = bottomTop + tooltipHeight <= viewportHeight - margin;
  const top =
    !canFitBelow && tooltipHeight > 0
      ? Math.max(margin, targetRect.top - tooltipHeight - gap)
      : Math.min(bottomTop, Math.max(margin, viewportHeight - tooltipHeight - margin));

  return { left, maxWidth, top };
}

/** 单行省略文本的统一展示组件；仅当真实溢出时显示完整内容 Tooltip。 */
export function OverflowTooltipText({
  as = "span",
  text,
  tooltipText,
  logArea,
  focusable = false,
  children,
  className,
  onBlur,
  onFocus,
  onMouseEnter,
  onMouseLeave,
  tabIndex,
  ...restProps
}: OverflowTooltipTextProps) {
  /** 目标文本节点，用于测量是否溢出和定位 Tooltip。 */
  const targetRef = useRef<HTMLElement | null>(null);
  /** Tooltip 节点用于二次测量，避免靠近视口边缘时被裁切。 */
  const tooltipRef = useRef<HTMLDivElement | null>(null);
  /** ARIA 描述 ID，仅在 Tooltip 可见时挂到目标文本上。 */
  const tooltipId = useId();
  /** 当前文本是否真的发生溢出，决定是否展示 Tooltip。 */
  const [isOverflowing, setIsOverflowing] = useState(false);
  /** Tooltip 可见性，悬停或聚焦时打开，离开或失焦时关闭。 */
  const [isTooltipVisible, setIsTooltipVisible] = useState(false);
  /** Tooltip 的 fixed 坐标，按目标元素实时计算。 */
  const [tooltipPosition, setTooltipPosition] = useState<TooltipPosition | null>(null);
  /** 每个实例最多写一次展示日志，避免鼠标扫过长列表时产生噪音。 */
  const hasLoggedShowRef = useRef(false);
  const resolvedTooltipText = tooltipText ?? text;
  const canShowTooltip = isOverflowing && resolvedTooltipText.trim().length > 0;

  /** 重新测量文本溢出状态；ResizeObserver 和展示前都会调用。 */
  const measureOverflow = useCallback(() => {
    const target = targetRef.current;
    const nextOverflowing = Boolean(target && resolvedTooltipText.trim() && isElementOverflowing(target));

    setIsOverflowing(nextOverflowing);

    if (!nextOverflowing) {
      setIsTooltipVisible(false);
    }

    return nextOverflowing;
  }, [resolvedTooltipText]);

  /** 重新计算浮层位置，滚动和窗口缩放时保持贴近目标文本。 */
  const updateTooltipPosition = useCallback(() => {
    const target = targetRef.current;

    if (!target) {
      return;
    }

    setTooltipPosition(getTooltipPosition(target, tooltipRef.current));
  }, []);

  /** 展示 Tooltip，并写入一次脱敏的 debug 日志。 */
  const showTooltip = useCallback(
    (trigger: TooltipTriggerSource) => {
      if (!measureOverflow()) {
        return;
      }

      updateTooltipPosition();
      setIsTooltipVisible(true);

      if (!hasLoggedShowRef.current) {
        hasLoggedShowRef.current = true;
        logDebug("显示溢出文本 Tooltip。", {
          category: "frontend",
          event: "overflow_tooltip_show",
          status: "shown",
          metadata: {
            area: logArea,
            element: as,
            trigger,
            textLength: resolvedTooltipText.length,
          },
        });
      }
    },
    [as, logArea, measureOverflow, resolvedTooltipText.length, updateTooltipPosition],
  );

  /** 关闭 Tooltip 的逻辑集中处理，避免多个事件入口各自维护状态。 */
  const hideTooltip = useCallback(() => {
    setIsTooltipVisible(false);
  }, []);

  useLayoutEffect(() => {
    measureOverflow();
  }, [measureOverflow, text]);

  useEffect(() => {
    const target = targetRef.current;

    if (!target) {
      return undefined;
    }

    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measureOverflow);

      return () => window.removeEventListener("resize", measureOverflow);
    }

    // ResizeObserver 只监听单个文本节点，成本稳定，避免每次鼠标悬停才触发强制布局。
    const observer = new ResizeObserver(() => measureOverflow());
    observer.observe(target);

    return () => observer.disconnect();
  }, [measureOverflow]);

  useEffect(() => {
    const target = targetRef.current;

    if (!target) {
      return undefined;
    }

    const interactionTarget = getTooltipInteractionTarget(target);
    const eventTargets = Array.from(new Set<HTMLElement>([target, interactionTarget]));
    // 原生监听兜底覆盖 Tauri WebView、列表按钮焦点和自动化工具的事件路径，React 回调仍保留给调用方扩展。
    const handleHoverStart = () => showTooltip("hover");
    const handleHoverEnd = () => hideTooltip();
    const handleFocusStart = () => showTooltip("focus");
    const handleFocusEnd = () => hideTooltip();
    // 部分 WebView/自动化点击会直接激活按钮但不派发 hover，点击兜底仍按焦点行为记日志。
    const handlePressStart = () => showTooltip("focus");

    eventTargets.forEach((eventTarget) => {
      eventTarget.addEventListener("pointerenter", handleHoverStart);
      eventTarget.addEventListener("pointerleave", handleHoverEnd);
      eventTarget.addEventListener("pointerdown", handlePressStart);
      eventTarget.addEventListener("mouseenter", handleHoverStart);
      eventTarget.addEventListener("mouseleave", handleHoverEnd);
      eventTarget.addEventListener("mousedown", handlePressStart);
      eventTarget.addEventListener("click", handlePressStart);
      eventTarget.addEventListener("focus", handleFocusStart);
      eventTarget.addEventListener("blur", handleFocusEnd);
      eventTarget.addEventListener("focusin", handleFocusStart);
      eventTarget.addEventListener("focusout", handleFocusEnd);
    });

    return () => {
      eventTargets.forEach((eventTarget) => {
        eventTarget.removeEventListener("pointerenter", handleHoverStart);
        eventTarget.removeEventListener("pointerleave", handleHoverEnd);
        eventTarget.removeEventListener("pointerdown", handlePressStart);
        eventTarget.removeEventListener("mouseenter", handleHoverStart);
        eventTarget.removeEventListener("mouseleave", handleHoverEnd);
        eventTarget.removeEventListener("mousedown", handlePressStart);
        eventTarget.removeEventListener("click", handlePressStart);
        eventTarget.removeEventListener("focus", handleFocusStart);
        eventTarget.removeEventListener("blur", handleFocusEnd);
        eventTarget.removeEventListener("focusin", handleFocusStart);
        eventTarget.removeEventListener("focusout", handleFocusEnd);
      });
    };
  }, [hideTooltip, showTooltip]);

  useLayoutEffect(() => {
    if (isTooltipVisible) {
      updateTooltipPosition();
    }
  }, [isTooltipVisible, updateTooltipPosition]);

  useEffect(() => {
    if (!isTooltipVisible) {
      return undefined;
    }

    window.addEventListener("resize", updateTooltipPosition);
    window.addEventListener("scroll", updateTooltipPosition, true);

    return () => {
      window.removeEventListener("resize", updateTooltipPosition);
      window.removeEventListener("scroll", updateTooltipPosition, true);
    };
  }, [isTooltipVisible, updateTooltipPosition]);

  const elementProps = {
    ...restProps,
    ref: targetRef,
    className,
    tabIndex: focusable && isOverflowing ? 0 : tabIndex,
    "aria-describedby": isTooltipVisible ? tooltipId : restProps["aria-describedby"],
    onMouseEnter: (event: MouseEvent<HTMLElement>) => {
      onMouseEnter?.(event);
      showTooltip("hover");
    },
    onMouseLeave: (event: MouseEvent<HTMLElement>) => {
      onMouseLeave?.(event);
      hideTooltip();
    },
    onFocus: (event: FocusEvent<HTMLElement>) => {
      onFocus?.(event);
      showTooltip("focus");
    },
    onBlur: (event: FocusEvent<HTMLElement>) => {
      onBlur?.(event);
      hideTooltip();
    },
  };

  return (
    <>
      {createElement(as, elementProps, children ?? text)}
      {isTooltipVisible &&
        canShowTooltip &&
        tooltipPosition &&
        createPortal(
          <div
            ref={tooltipRef}
            id={tooltipId}
            className="overflow-tooltip"
            role="tooltip"
            style={{
              left: tooltipPosition.left,
              maxWidth: tooltipPosition.maxWidth,
              top: tooltipPosition.top,
            }}
          >
            {resolvedTooltipText}
          </div>,
          document.body,
        )}
    </>
  );
}
