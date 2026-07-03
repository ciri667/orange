import { useEffect, useRef, type RefObject } from "react";

/** 无遮罩浮层（popover / 下拉菜单）通用的关闭控制配置。 */
export interface DismissableOptions {
  /** 触发点击外部关闭的事件，默认 mousedown，与现有遮罩弹窗保持一致。 */
  event?: "mousedown" | "click";
  /** 是否响应 Esc 键关闭，默认 true。 */
  escape?: boolean;
}

/**
 * 为无遮罩浮层（popover / 下拉菜单）统一提供「点击外部关闭 + Esc 关闭」。
 *
 * 返回一个 ref，应挂在「触发按钮 + 浮层」共同的最近包裹容器上，
 * hook 据此判定点击是否落在浮层内部还是外部。
 *
 * 与带遮罩的真模态（ConfirmDialog/SkillsModal 等）不同，popover 没有遮罩层，
 * 用户切到别的板块时浮层仍会悬在原处，因此需要监听 document 主动关闭。
 *
 * 泛型 `TElement` 用于让 ref 精确匹配挂载目标（如 `<div>` 期望 HTMLDivElement）。
 * 传入 `externalRef` 时复用调用方已有的 ref（如多个浮层共用同一外层容器），
 * 否则使用 hook 内部 ref；返回的 ref 始终可用于挂载（externalRef 模式下挂载无副作用）。
 *
 * 仅在 `open` 为真时挂载监听，避免误关其他状态；卸载或关闭后自动解绑。
 */
export function useDismissable<TElement extends HTMLElement = HTMLElement>(
  open: boolean,
  onDismiss: () => void,
  options: DismissableOptions & { externalRef?: RefObject<TElement | null> } = {},
): RefObject<TElement | null> {
  const internalRef = useRef<TElement | null>(null);
  // 始终保持最新的回调，避免回调闭包捕获过期状态。
  const onDismissRef = useRef(onDismiss);
  onDismissRef.current = onDismiss;

  const eventType = options.event ?? "mousedown";
  const escapeEnabled = options.escape !== false;

  useEffect(() => {
    if (!open) {
      return;
    }

    const container = options.externalRef?.current ?? internalRef.current;

    /** 点击落在浮层容器外部时关闭浮层。 */
    function handlePointerDown(event: MouseEvent) {
      if (!(event.target instanceof Node)) {
        return;
      }

      // 容器未挂载或点击不在容器内，视为外部点击，触发关闭。
      if (!container || !container.contains(event.target)) {
        onDismissRef.current();
      }
    }

    /** Esc 键关闭浮层，并阻止默认行为以免影响编辑器输入。 */
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        onDismissRef.current();
      }
    }

    document.addEventListener(eventType, handlePointerDown, true);
    if (escapeEnabled) {
      document.addEventListener("keydown", handleKeyDown, true);
    }

    return () => {
      document.removeEventListener(eventType, handlePointerDown, true);
      if (escapeEnabled) {
        document.removeEventListener("keydown", handleKeyDown, true);
      }
    };
  }, [open, eventType, escapeEnabled]);

  return internalRef;
}
