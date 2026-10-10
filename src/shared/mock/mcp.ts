import type { McpApprovalRequest, McpOverview, McpProbeResult, McpServerDraft, McpServerView } from "../types";

/** 深拷贝，避免调用方拿到 mock 内部引用后绕过命令直接改状态。 */
function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** 按草稿生成一台服务器的模拟视图；启用的服务器视为已连上并带两个示例工具。 */
function serverFromDraft(draft: McpServerDraft): McpServerView {
  const isHttp = draft.transport === "http";
  const args = draft.argsText
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const enabled = draft.enabled;
  return {
    name: draft.name,
    enabled,
    transport: draft.transport,
    state: enabled ? "ready" : "disabled",
    command: isHttp ? "" : draft.command.trim(),
    args: isHttp ? [] : args,
    url: isHttp ? draft.url.trim() : "",
    cwd: isHttp ? draft.cwd.trim() : "",
    timeoutSeconds: draft.timeoutSeconds,
    hasAuthorization: typeof draft.authorizationToken === "string" && draft.authorizationToken.trim().length > 0,
    error: null,
    stderrTail: "",
    // 浏览器模拟没有真实服务器，固定给一个只读和一个写入示例工具。
    tools: enabled
      ? [
          {
            rawName: "read_example",
            publicName: `mcp__${draft.name.replace(/-/g, "_")}__read_example`,
            description: "示例只读工具（浏览器模拟）",
            readOnly: true,
            exposed: true,
          },
          {
            rawName: "write_example",
            publicName: `mcp__${draft.name.replace(/-/g, "_")}__write_example`,
            description: "示例写入工具（浏览器模拟，需要确认）",
            readOnly: false,
            exposed: true,
          },
        ]
      : [],
    exposedToolCount: enabled ? 2 : 0,
  };
}

/** 浏览器开发态的 MCP 内存状态；桌面端不走这里。 */
export const browserMcpMock: { overview: McpOverview } = {
  overview: {
    configPath: "~/.orange/mcp.json",
    servers: [
      {
        name: "fs-demo",
        enabled: true,
        transport: "stdio",
        state: "ready",
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-filesystem", "D:\\mcp-demo"],
        url: "",
        cwd: "",
        timeoutSeconds: 60,
        hasAuthorization: false,
        error: null,
        stderrTail: "",
        tools: [
          {
            rawName: "read_file",
            publicName: "mcp__fs_demo__read_file",
            description: "读取授权目录内的文件（示例）",
            readOnly: true,
            exposed: true,
          },
          {
            rawName: "write_file",
            publicName: "mcp__fs_demo__write_file",
            description: "写入授权目录内的文件（示例，需要确认）",
            readOnly: false,
            exposed: true,
          },
        ],
        exposedToolCount: 2,
      },
      {
        name: "internal-api",
        enabled: false,
        transport: "http",
        state: "disabled",
        command: "",
        args: [],
        url: "https://mcp.example.com/rpc",
        cwd: "",
        timeoutSeconds: 30,
        hasAuthorization: false,
        error: null,
        stderrTail: "",
        tools: [],
        exposedToolCount: 0,
      },
    ],
    issues: [],
  },
};

/** 校验草稿，规则和 Rust validate_draft 保持一致，让浏览器开发态也能看到错误提示。 */
function validateDraft(draft: McpServerDraft): void {
  const name = draft.name.trim();
  if (name.length === 0) {
    throw new Error("请填写服务器名。");
  }
  if (name.length > 64 || !/^[A-Za-z0-9_-]+$/.test(name)) {
    throw new Error("服务器名只能包含字母、数字、_ 和 -。");
  }
  if (draft.transport === "http") {
    if (!/^https?:\/\//.test(draft.url.trim())) {
      throw new Error("url 必须以 http:// 或 https:// 开头。");
    }
  } else if (draft.command.trim().length === 0) {
    throw new Error("请填写要启动的命令。");
  }
  if (draft.timeoutSeconds < 1 || draft.timeoutSeconds > 300) {
    throw new Error("timeout 必须在 1 到 300 秒之间。");
  }
}

/** 读取模拟概览。 */
export function mockLoadMcpOverview(): McpOverview {
  return clone(browserMcpMock.overview);
}

/** 新增或覆盖一台服务器；启用的条目重新置为 ready。 */
export function mockSaveMcpServer(draft: McpServerDraft): McpOverview {
  validateDraft(draft);
  const view = serverFromDraft(draft);
  const others = browserMcpMock.overview.servers.filter((server) => server.name !== draft.name);
  // 编辑时保留原有令牌状态：草稿没带新令牌就沿用之前的 hasAuthorization。
  const previous = browserMcpMock.overview.servers.find((server) => server.name === draft.name);
  if (previous && draft.authorizationToken === undefined) {
    view.hasAuthorization = previous.hasAuthorization;
  }
  browserMcpMock.overview.servers = [...others, view];
  return mockLoadMcpOverview();
}

/** 删除一台服务器。 */
export function mockRemoveMcpServer(name: string): McpOverview {
  const found = browserMcpMock.overview.servers.some((server) => server.name === name);
  if (!found) {
    throw new Error(`没有名为 ${name} 的 MCP 服务器。`);
  }
  browserMcpMock.overview.servers = browserMcpMock.overview.servers.filter((server) => server.name !== name);
  return mockLoadMcpOverview();
}

/** 只切换启用状态；禁用后示例工具立即清空。 */
export function mockSetMcpServerEnabled(name: string, enabled: boolean): McpOverview {
  const servers = browserMcpMock.overview.servers.map((server) => {
    if (server.name !== name) {
      return server;
    }
    const next: McpServerView = { ...server, enabled, state: enabled ? "ready" : "disabled" };
    if (!enabled) {
      next.tools = [];
      next.exposedToolCount = 0;
    } else if (server.tools.length === 0) {
      next.tools = serverFromDraft({
        name: server.name,
        enabled: true,
        transport: server.transport === "http" ? "http" : "stdio",
        command: server.command,
        argsText: server.args.join("\n"),
        url: server.url,
        cwd: server.cwd,
        timeoutSeconds: server.timeoutSeconds,
        envText: "",
        headersText: "",
        disabledTools: [],
      }).tools;
      next.exposedToolCount = next.tools.length;
    }
    return next;
  });
  if (!servers.some((server) => server.name === name)) {
    throw new Error(`没有名为 ${name} 的 MCP 服务器。`);
  }
  browserMcpMock.overview.servers = servers;
  return mockLoadMcpOverview();
}

/** 试连模拟：启用的服务器返回示例工具名。 */
export function mockTestMcpServer(name: string): McpProbeResult {
  const server = browserMcpMock.overview.servers.find((item) => item.name === name);
  if (!server) {
    throw new Error(`没有名为 ${name} 的 MCP 服务器。`);
  }
  return {
    serverName: name,
    ok: true,
    message: `已连接，发现 ${server.tools.length} 个工具。`,
    toolNames: server.tools.map((tool) => tool.rawName),
  };
}

/* ---------- 确认事件的浏览器模拟通道 ---------- */

const approvalListeners = new Set<(request: McpApprovalRequest) => void>();

/** 注册确认请求回调；返回取消注册函数。 */
export function subscribeBrowserMockMcpApproval(listener: (request: McpApprovalRequest) => void): () => void {
  approvalListeners.add(listener);
  return () => {
    approvalListeners.delete(listener);
  };
}

/**
 * 触发一次模拟确认请求。
 * 浏览器开发态没有 Tauri 事件，可在 devtools 里执行
 * `window.dispatchEvent(new CustomEvent("orange-mock-mcp-approval", { detail: { approvalId: "demo-1", sessionId: "s1", serverName: "fs-demo", toolName: "write_file", argumentsPreview: '{"path":"a.md"}' } }))`
 * 来验证弹窗流程。
 */
export function emitBrowserMockMcpApproval(request: McpApprovalRequest): void {
  approvalListeners.forEach((listener) => listener(request));
}

if (typeof window !== "undefined") {
  window.addEventListener("orange-mock-mcp-approval", (event) => {
    const detail = (event as CustomEvent<McpApprovalRequest>).detail;
    if (detail && typeof detail.approvalId === "string") {
      emitBrowserMockMcpApproval(detail);
    }
  });
}
