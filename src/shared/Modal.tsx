import type { FormEventHandler, ReactNode } from "react";
import { cn } from "./cn";

/**
 * 全屏模态遮罩。
 * 点击空白处关闭；面板内部需要自行 stopPropagation，避免误关。
 */
export function ModalBackdrop({
  children,
  onClose,
  className,
}: {
  children: ReactNode;
  onClose: () => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "fixed inset-0 z-modal grid isolate place-items-center",
        "bg-[radial-gradient(circle_at_top,rgba(255,255,255,0.12),transparent_360px),rgba(23,23,23,0.46)]",
        "backdrop-blur-[8px] max-[760px]:p-2.5",
        className,
      )}
      role="presentation"
      onMouseDown={onClose}
    >
      {children}
    </div>
  );
}

/** 小型表单弹窗面板，用于重命名、新建文件等单字段对话框。 */
export function ModalForm({
  children,
  className,
  "aria-label": ariaLabel,
  onSubmit,
}: {
  children: ReactNode;
  className?: string;
  "aria-label": string;
  onSubmit: FormEventHandler<HTMLFormElement>;
}) {
  return (
    <form
      className={cn(
        "grid w-[min(420px,calc(100vw-40px))] isolate gap-3.5 rounded-panel border border-border-translucent bg-surface-translucent-strong p-4 shadow-app max-[760px]:w-[min(100%,calc(100vw-20px))]",
        className,
      )}
      aria-label={ariaLabel}
      onMouseDown={(event) => event.stopPropagation()}
      onSubmit={onSubmit}
    >
      {children}
    </form>
  );
}
