import { type InputHTMLAttributes } from "react";
import { cn } from "./cn";

/**
 * 自定义勾选框。
 * 原生 input 视觉隐藏，用后续 span 画方框；依赖 peer 才能显示选中和焦点。
 */
export function Checkbox({
  className,
  boxClassName,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { boxClassName?: string }) {
  return (
    <>
      <input type="checkbox" className={cn("peer sr-only", className)} {...props} />
      <span
        aria-hidden="true"
        className={cn(
          "relative inline-grid size-[18px] shrink-0 place-items-center rounded-[5px] border border-border-strong bg-white text-white transition-[background,border-color,box-shadow] duration-[160ms]",
          "after:h-1 after:w-2 after:-translate-y-px after:-rotate-45 after:border-b-2 after:border-l-2 after:border-current after:opacity-0 after:content-['']",
          "peer-checked:border-agent peer-checked:bg-agent peer-checked:after:opacity-100",
          "peer-focus-visible:shadow-[0_0_0_3px_var(--control-ring)]",
          "peer-disabled:border-[rgba(213,203,189,0.7)] peer-disabled:bg-[#f4f1eb]",
          boxClassName,
        )}
      />
    </>
  );
}
