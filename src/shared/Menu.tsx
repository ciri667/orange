import { type ButtonHTMLAttributes, type ReactNode } from "react";
import { cn } from "./cn";
import { useDismissable } from "./useDismissable";

/** 菜单浮层展开方向。文件树行默认向上，以免被滚动容器裁切。 */
export type MenuPanelPlacement = "bottom-end" | "top-end";

/**
 * 弹出菜单容器。
 * 把触发器和浮层包在一起，点击外部或按 Esc 时关闭。
 */
export function Menu({
  open,
  onClose,
  children,
  className,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  className?: string;
}) {
  const containerRef = useDismissable<HTMLDivElement>(open, onClose);

  return (
    <div ref={containerRef} className={cn("relative inline-flex overflow-visible", className)}>
      {children}
    </div>
  );
}

/** 菜单浮层。bottom-end 向下展开，top-end 向上展开。 */
export function MenuPanel({
  children,
  className,
  placement = "bottom-end",
}: {
  children: ReactNode;
  className?: string;
  placement?: MenuPanelPlacement;
}) {
  return (
    <div
      role="menu"
      className={cn(
        "absolute z-menu grid min-w-max max-w-[min(260px,calc(100vw-32px))] overflow-hidden rounded-control border border-border bg-surface p-[5px] shadow-app-soft",
        placement === "bottom-end" && "top-[calc(100%+8px)] right-0",
        placement === "top-end" && "right-0 bottom-[30px]",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** 菜单项。danger 用于删除；nested 用于子菜单缩进。 */
export function MenuItem({
  tone = "default",
  nested = false,
  className,
  type = "button",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: "default" | "danger";
  nested?: boolean;
}) {
  return (
    <button
      type={type}
      role="menuitem"
      className={cn(
        "inline-flex min-h-8 items-center justify-start gap-2 rounded-small border-0 bg-transparent px-[9px] text-left text-xs text-ink hover:bg-surface-hover",
        tone === "danger" && "text-danger",
        nested && "pl-[26px] text-ink-muted",
        className,
      )}
      {...props}
    />
  );
}
