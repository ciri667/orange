import { invokeLogged, isTauriRuntime } from "./runtime";
import {
  mockLoadMcpOverview,
  mockRemoveMcpServer,
  mockSaveMcpServer,
  mockSetMcpServerEnabled,
  mockTestMcpServer,
} from "../mock/mcp";
import type {
  McpOverview,
  McpProbeResult,
  McpServerDraft,
} from "../types";

/** 读取 MCP 服务器、连接状态和配置错误。 */
export async function loadMcpOverview(): Promise<McpOverview> {
  if (!isTauriRuntime()) {
    return mockLoadMcpOverview();
  }

  return invokeLogged<McpOverview>("load_mcp_overview");
}

/** 新增或覆盖一台服务器；桌面端会写 ~/.orange/mcp.json 并重连已启用条目。 */
export async function saveMcpServer(draft: McpServerDraft): Promise<McpOverview> {
  if (!isTauriRuntime()) {
    return mockSaveMcpServer(draft);
  }

  return invokeLogged<McpOverview>("save_mcp_server", { draft });
}

/** 删除一台服务器并重连其余条目。 */
export async function removeMcpServer(name: string): Promise<McpOverview> {
  if (!isTauriRuntime()) {
    return mockRemoveMcpServer(name);
  }

  return invokeLogged<McpOverview>("remove_mcp_server", { name });
}

/** 只切换启用状态；关闭后该服务器的工具立刻从模型列表消失。 */
export async function setMcpServerEnabled(name: string, enabled: boolean): Promise<McpOverview> {
  if (!isTauriRuntime()) {
    return mockSetMcpServerEnabled(name, enabled);
  }

  return invokeLogged<McpOverview>("set_mcp_server_enabled", { name, enabled });
}

/** 按磁盘上的 mcp.json 重连。 */
export async function reloadMcpServers(): Promise<McpOverview> {
  if (!isTauriRuntime()) {
    return mockLoadMcpOverview();
  }

  return invokeLogged<McpOverview>("reload_mcp_servers");
}

/** 试连一次，不改变启用状态。 */
export async function testMcpServer(name: string): Promise<McpProbeResult> {
  if (!isTauriRuntime()) {
    return mockTestMcpServer(name);
  }

  return invokeLogged<McpProbeResult>("test_mcp_server", { name });
}
