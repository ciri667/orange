import { listen } from "@tauri-apps/api/event";
import { invokeLogged, isTauriRuntime } from "./runtime";
import {
  mockLoadMcpOverview,
  mockRemoveMcpServer,
  mockSaveMcpServer,
  mockSetMcpServerEnabled,
  mockTestMcpServer,
  subscribeBrowserMockMcpApproval,
} from "../mock/mcp";
import type {
  McpApprovalDecision,
  McpApprovalRequest,
  McpOverview,
  McpProbeResult,
  McpServerDraft,
} from "../types";

/** 后端发出确认请求的事件名，和 Rust MCP_APPROVAL_EVENT 对齐。 */
const MCP_APPROVAL_EVENT = "mcp-tool-approval";

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

/** 把确认弹窗的选择送回正在等待的回合。 */
export async function decideMcpToolCall(decision: McpApprovalDecision): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }

  return invokeLogged<void>("decide_mcp_tool_call", { decision });
}

/**
 * 订阅 MCP 工具确认请求。
 * 浏览器开发态走 window 自定义事件通道（见 mock/mcp.ts），方便本地验证弹窗。
 */
export async function subscribeMcpApprovals(onRequest: (request: McpApprovalRequest) => void): Promise<() => void> {
  if (!isTauriRuntime()) {
    return subscribeBrowserMockMcpApproval(onRequest);
  }

  const unlisten = await listen<McpApprovalRequest>(MCP_APPROVAL_EVENT, (event) => {
    onRequest(event.payload);
  });
  return unlisten;
}
