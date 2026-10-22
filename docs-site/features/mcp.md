# MCP 服务

橘记支持通过 [Model Context Protocol](https://modelcontextprotocol.io) 接入外部工具服务。

## 配置文件

MCP 服务器配置在橘记用户目录下的 `mcp.json`：

```
~/.orange/mcp.json
```

结构：

```json
{
  "mcpServers": {
    "my-server": {
      "command": "npx",
      "args": ["-y", "@some/mcp-server"],
      "env": {
        "API_KEY": "${MY_API_KEY}"
      }
    },
    "remote-server": {
      "url": "https://example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${TOKEN}"
      }
    }
  }
}
```

## 传输方式

| 传输 | 配置字段 | 说明 |
| --- | --- | --- |
| stdio | `command` + `args` | 本地进程，通过标准输入输出通信 |
| streamable HTTP | `url` | 远程服务 |

::: warning 配置约束
- `command` 必须是**一个可执行文件**，不能是 shell 脚本（不能包含换行）。
- 不支持旧的 SSE 传输，请改用 streamable HTTP 的 `url`。
- 不支持 `!command` 插值，请改用 `${环境变量}`。
:::

## 权限与审批

| 场景 | MCP 是否可见 |
| --- | --- |
| 本地交互会话，进阶级别及以上 | ✅ |
| 基础级别会话 | ❌ |
| 计划模式 | ❌ |
| 远程 / IM 会话 | ❌ |

- **只读工具**直接执行。
- **写入类工具**在非完全级别下需要你逐次审批。
- 与橘记内置工具重名的 MCP 工具不会暴露给模型。

## 下一步

- [Agent 工具](/features/agent-tools)
- [设置项总览](/reference/settings)
