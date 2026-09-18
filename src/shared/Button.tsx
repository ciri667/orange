import { forwardRef, type ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

/** 按钮视觉变体，对应原先 primary / ghost / text / icon class。 */
export type ButtonVariant = "primary" | "ghost" | "text" | "icon";

/** compact 用于工具栏和弹窗操作，默认高度用于主行动点。 */
export type ButtonSize = "default" | "compact";

/** danger 用于删除、拒绝、移除授权等不可直接撤销的操作。 */
export type ButtonTone = "default" | "danger";

/** 应用内按钮属性，在原生 button 之上增加变体、尺寸和语义色。 */
export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  tone?: ButtonTone;
}

const baseClassName =
  "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-control border border-solid transition-[background,border-color,color] duration-[140ms]";

const variantClassName: Record<ButtonVariant, string> = {
  primary: "min-h-9 border-transparent bg-accent px-3.5 font-semibold text-white hover:enabled:bg-accent-strong",
  ghost: "min-h-8 border-transparent bg-transparent px-3 text-ink hover:enabled:bg-surface-hover",
  text: "min-h-8 border-transparent bg-transparent px-3 text-ink hover:enabled:bg-surface-hover",
  icon: "size-8 min-h-8 border-transparent bg-transparent p-0 text-ink-muted hover:enabled:bg-surface-hover hover:enabled:text-ink",
};

/** 统一按钮。应用内行动点用这个组件。 */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "ghost", size = "default", tone = "default", type = "button", className, ...props },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      data-button=""
      className={cn(
        baseClassName,
        variantClassName[variant],
        size === "compact" && variant !== "icon" && "min-h-8 text-xs",
        size === "compact" && variant === "ghost" && "px-2 py-1.5 text-xs",
        size === "compact" && variant === "icon" && "size-7 min-h-7",
        tone === "danger" && variant === "primary" && "bg-danger text-white hover:enabled:bg-danger",
        tone === "danger" && variant !== "primary" && "text-danger",
        className,
      )}
      {...props}
    />
  );
});
