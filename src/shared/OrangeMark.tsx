import { cn } from "./cn";

/** 所有品牌入口共用原版橘子插画，避免小尺寸标记与应用图标出现两套形象。 */
export function OrangeMark({ className }: { className?: string }) {
  return (
    <img className={cn("block shrink-0 object-contain", className)} src="/orange-logo.svg" alt="" aria-hidden="true" draggable={false} />
  );
}
