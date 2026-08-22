import type { ReactNode } from "react";
import { cn } from "./cn";

/** 启动全屏壳的三种用途：加载中、初始化失败、尚未连接知识库。 */
export type BootScreenVariant = "loading" | "error" | "empty";

/**
 * 启动与空状态全屏壳。这是 Tailwind 渐进接入的参考面：
 * 只用 token 工具类，不再依赖 app.css 里的 loading-shell / empty-shell。
 */
export function BootScreen({
  variant = "loading",
  children,
}: {
  variant?: BootScreenVariant;
  children: ReactNode;
}) {
  return (
    <main
      className={cn(
        "grid h-full w-full place-items-center gap-4 bg-app text-ink-muted",
        "bg-[linear-gradient(180deg,rgba(var(--surface-rgb),0.58),rgba(var(--paper-rgb),0)_210px)]",
        variant !== "loading" && "content-center p-12 text-center",
      )}
    >
      {children}
    </main>
  );
}

/** 空状态主标题，对应原先 .empty-shell h1。 */
export function BootTitle({ children }: { children: ReactNode }) {
  return <h1 className="mt-2.5 mb-0 max-w-[680px] text-[34px] leading-[1.18] text-ink-strong">{children}</h1>;
}

/** 空状态说明文字，对应原先 .empty-shell p。 */
export function BootCopy({ children }: { children: ReactNode }) {
  return <p className="m-0 max-w-[620px] text-[15px] leading-[1.7]">{children}</p>;
}

/** 启动失败详情，对应原先 .boot-error-message。 */
export function BootErrorMessage({ children }: { children: ReactNode }) {
  return (
    <p className="m-0 max-w-[680px] rounded-[7px] border border-danger/30 bg-danger-soft px-3 py-2.5 text-[13px] leading-[1.55] text-danger [overflow-wrap:anywhere]">
      {children}
    </p>
  );
}
