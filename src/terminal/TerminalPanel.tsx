import { useEffect, useRef } from "react";
import { Button } from "../shared/Button";
import { terminalResize } from "../shared/tauriApi";
import type { TerminalContent } from "./useTerminals";
import "@xterm/xterm/css/xterm.css";

/** 隐藏时保留同一个终端 DOM；重新显示后按真实尺寸 fit 并通知 PTY。 */
export function TerminalPanel({ content, active, onRestart }: { content: TerminalContent; active: boolean; onRestart: () => void }) {
  const container = useRef<HTMLDivElement>(null);
  const opened = useRef(false);
  useEffect(() => {
    if (!container.current || opened.current) return;
    content.terminal.open(container.current);
    opened.current = true;
  }, [content]);
  useEffect(() => {
    const element = container.current;
    if (!element || !active) return;
    let frame = 0;
    const fit = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (!element.clientWidth || !element.clientHeight) return;
        content.fit.fit();
        if (content.descriptor && content.status === "running") void terminalResize(content.descriptor.terminalId, content.terminal.cols, content.terminal.rows).catch(() => undefined);
      });
    };
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    fit();
    content.terminal.focus();
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, [active, content.descriptor, content.status, content]);
  return <section className="terminal-panel" hidden={!active} aria-label="交互终端">
    <div className="terminal-toolbar">
      <span title={content.descriptor?.cwd}>{content.descriptor ? `${content.descriptor.shell} · ${content.descriptor.cwd}` : content.status === "failed" ? "终端创建失败" : "正在创建终端…"}</span>
      {(content.status === "exited" || content.status === "failed") && <Button size="compact" onClick={onRestart}>重新启动</Button>}
    </div>
    {content.error && <p className="terminal-notice is-error" role="alert">{content.error}</p>}
    {content.status === "exited" && <p className="terminal-notice">终端已退出{content.exitCode != null ? `（${content.exitCode}）` : ""}</p>}
    <div className="terminal-screen" ref={container} />
  </section>;
}
