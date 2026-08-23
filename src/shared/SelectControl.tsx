import { type SelectHTMLAttributes } from "react";
import { cn } from "./cn";

/**
 * 带自定义箭头的 select。
 * 全局 appearance:none 之后，原生下拉箭头会消失，这里补回。
 */
export function SelectControl({ className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <span className="relative grid w-full min-w-0">
      <select
        className={cn(
          "w-full min-w-0 min-h-[var(--control-height)] appearance-none rounded-control border border-border bg-control px-[11px] pr-[34px] text-[13px] font-normal leading-[var(--control-height)] text-ink",
          "hover:border-border-strong hover:bg-control-hover",
          "focus-visible:border-[rgba(var(--primary-rgb),0.46)] focus-visible:outline-[3px] focus-visible:outline-[var(--control-ring)] focus-visible:outline-offset-0",
          className,
        )}
        {...props}
      />
      <span
        aria-hidden="true"
        className="pointer-events-none absolute top-1/2 right-3 h-[7px] w-[7px] -translate-y-[68%] rotate-45 border-r-[1.6px] border-b-[1.6px] border-current text-ink-muted"
      />
    </span>
  );
}
