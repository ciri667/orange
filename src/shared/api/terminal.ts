import { Channel } from "@tauri-apps/api/core";
import { invokeLogged, isTauriRuntime } from "./runtime";
import type { TerminalDescriptor, TerminalEvent } from "../types";

/** 输出 Channel 在创建进程前注册，首帧输出不会因为 DOM 还没挂载而丢失。 */
export async function createTerminal(knowledgeBaseId: string, cols: number, rows: number, onEvent: (event: TerminalEvent) => void): Promise<TerminalDescriptor> {
  if (!isTauriRuntime()) throw new Error("交互终端仅在橘记桌面端可用。");
  const channel = new Channel<TerminalEvent>();
  channel.onmessage = onEvent;
  return invokeLogged<TerminalDescriptor>("terminal_create", { knowledgeBaseId, cols, rows, onEvent: channel });
}
export const terminalInput = (terminalId: string, data: string) => invokeLogged<void>("terminal_input", { terminalId, data });
export const terminalResize = (terminalId: string, cols: number, rows: number) => invokeLogged<void>("terminal_resize", { terminalId, cols, rows });
export const closeTerminal = (terminalId: string) => invokeLogged<void>("terminal_close", { terminalId });
