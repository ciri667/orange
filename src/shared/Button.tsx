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
  "inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-control border border-solid transition-[background,border-color,color,box-shadow] duration-[160ms]";

const variantClassName: Record<ButtonVariant, string> = {
  primary: "min-h-10 border-transparent bg-accent px-3.5 font-bold text-white hover:enabled:bg-accent-strong",
  ghost:
    "min-h-[34px] border-border-translucent bg-surface-translucent px-3.5 text-ink hover:enabled:border-border-strong hover:enabled:bg-surface-hover",
  text: "min-h-[34px] border-border-translucent bg-surface-translucent px-3.5 text-ink hover:enabled:border-border-strong hover:enabled:bg-surface-hover",
  icon: "size-[34px] min-h-[34px] border-border-translucent bg-surface-translucent p-0 text-ink hover:enabled:border-border-strong hover:enabled:bg-surface-hover",
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
        size === "compact" && variant !== "icon" && "min-h-[34px]",
        size === "compact" && variant === "ghost" && "px-[9px] py-1.5 text-xs",
        size === "compact" && variant === "icon" && "size-[26px] min-h-[26px]",
        tone === "danger" && variant === "primary" && "bg-danger text-white hover:enabled:bg-danger",
        tone === "danger" && variant !== "primary" && "text-danger",
        className,
      )}
      {...props}
    />
  );
});
