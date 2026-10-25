import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { closeTerminal, createTerminal, terminalInput } from "../shared/tauriApi";
import type { TerminalDescriptor } from "../shared/types";

/**
 * xterm 画布不能直接消费 CSS 变量，创建实例时把 app.css 的终端 token 读成具体颜色。
 * 与 Codex 桌面端一致：底色、前景和 ANSI 色都跟随应用主题，而不是系统终端配色。
 */
export function terminalTheme(): ITheme {
  const styles = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) => styles.getPropertyValue(name).trim() || fallback;
  return {
    background: read("--terminal-bg", "#ffffff"),
    foreground: read("--terminal-fg", "#292724"),
    cursor: read("--terminal-cursor", "#c6531b"),
    cursorAccent: read("--terminal-cursor-accent", "#ffffff"),
    selectionBackground: read("--terminal-selection", "rgba(198, 83, 27, 0.18)"),
    selectionInactiveBackground: read("--terminal-selection-inactive", "rgba(198, 83, 27, 0.10)"),
    black: read("--terminal-ansi-black", "#57606a"),
    red: read("--terminal-ansi-red", "#cf222e"),
    green: read("--terminal-ansi-green", "#116329"),
    yellow: read("--terminal-ansi-yellow", "#9a6700"),
    blue: read("--terminal-ansi-blue", "#0550ae"),
    magenta: read("--terminal-ansi-magenta", "#8250df"),
    cyan: read("--terminal-ansi-cyan", "#1b7c83"),
    white: read("--terminal-ansi-white", "#292724"),
    brightBlack: read("--terminal-ansi-bright-black", "#6e7781"),
    brightRed: read("--terminal-ansi-bright-red", "#b42318"),
    brightGreen: read("--terminal-ansi-bright-green", "#067647"),
    brightYellow: read("--terminal-ansi-bright-yellow", "#7d4e00"),
    brightBlue: read("--terminal-ansi-bright-blue", "#218bff"),
    brightMagenta: read("--terminal-ansi-bright-magenta", "#a475f9"),
    brightCyan: read("--terminal-ansi-bright-cyan", "#3192aa"),
    brightWhite: read("--terminal-ansi-bright-white", "#201f1c"),
  };
}

export interface TerminalContent {
  id: string;
  knowledgeBaseId: string;
  terminal: Terminal;
  fit: FitAddon;
  descriptor?: TerminalDescriptor;
  status: "starting" | "running" | "exited" | "failed";
  exitCode?: number | null;
  error?: string;
  generation: number;
  pendingInput: string[];
}

/** PTY 与 xterm 实例按内容页签持有，不随面板显隐卸载。 */
export function useTerminals() {
  const contents = useRef(new Map<string, TerminalContent>());
  const [, refresh] = useState(0);
  const changed = () => refresh((current) => current + 1);
  const start = useCallback(async (content: TerminalContent) => {
    const generation = ++content.generation;
    content.status = "starting";
    content.error = undefined;
    content.pendingInput = [];
    content.terminal.options.disableStdin = true;
    changed();
    try {
      const descriptor = await createTerminal(content.knowledgeBaseId, content.terminal.cols, content.terminal.rows, (event) => {
        if (content.generation !== generation || !contents.current.has(content.id)) return;
        if (event.kind === "output") content.terminal.write(new Uint8Array(event.data));
        else {
          content.status = event.kind === "exit" ? "exited" : "failed";
          content.exitCode = event.kind === "exit" ? event.code : null;
          content.error = event.kind === "error" ? event.message : undefined;
          content.terminal.options.disableStdin = true;
          changed();
        }
      });
      if (content.generation !== generation || !contents.current.has(content.id)) { await closeTerminal(descriptor.terminalId); return; }
      content.descriptor = descriptor;
      if (content.status === "starting") { content.status = "running"; content.terminal.options.disableStdin = false; }
      // ConPTY/PowerShell 启动时会询问光标位置；xterm 在 descriptor 返回前的回应也必须送达。
      for (const data of content.pendingInput.splice(0)) await terminalInput(descriptor.terminalId, data);
    } catch (cause) {
      if (content.generation !== generation || !contents.current.has(content.id)) return;
      content.status = "failed";
      content.error = cause instanceof Error ? cause.message : String(cause);
    }
    changed();
  }, []);

  const create = useCallback((knowledgeBaseId: string) => {
    const id = crypto.randomUUID();
    const terminal = new Terminal({ fontSize: 13, fontFamily: "Cascadia Mono, Consolas, monospace", cursorBlink: true, scrollback: 10000, disableStdin: true, theme: terminalTheme() });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    const content: TerminalContent = { id, knowledgeBaseId, terminal, fit, status: "starting", generation: 0, pendingInput: [] };
    contents.current.set(id, content);
    let inputQueue = Promise.resolve();
    terminal.onData((data) => {
      const descriptor = content.descriptor;
      if (!descriptor && content.status === "starting") { content.pendingInput.push(data); return; }
      if (!descriptor || content.status !== "running") return;
      inputQueue = inputQueue.then(() => terminalInput(descriptor.terminalId, data)).catch((cause) => { content.error = String(cause); changed(); });
    });
    void start(content);
    changed();
    return id;
  }, [start]);

  const close = useCallback(async (id: string) => {
    const content = contents.current.get(id);
    if (!content) return;
    // 创建仍在途时用代计数拒绝迟到结果，并由创建回调回收新进程。
    ++content.generation;
    if (content.descriptor) await closeTerminal(content.descriptor.terminalId);
    contents.current.delete(id);
    content.terminal.dispose();
    changed();
  }, []);

  const restart = useCallback(async (id: string) => {
    const content = contents.current.get(id);
    if (!content || content.status === "running" || content.status === "starting") return;
    if (content.descriptor) await closeTerminal(content.descriptor.terminalId);
    content.descriptor = undefined;
    content.terminal.writeln("\r\n── 重新启动终端 ──\r\n");
    await start(content);
  }, [start]);

  useEffect(() => () => {
    for (const content of contents.current.values()) {
      ++content.generation;
      content.terminal.dispose();
      if (content.descriptor) void closeTerminal(content.descriptor.terminalId).catch(() => undefined);
    }
    contents.current.clear();
  }, []);
  return { contents: [...contents.current.values()], get: (id: string) => contents.current.get(id), create, close, restart };
}
export type Terminals = ReturnType<typeof useTerminals>;
