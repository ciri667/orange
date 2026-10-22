# 项目结构

橘记是 **Vite + React + TypeScript** 前端加 **Tauri v2 Rust** 桌面后端。

```
orange/
├── src/                  # 应用 UI，按功能拆分
│   ├── workspace/        # 工作台编排
│   ├── knowledge-base/   # 知识库
│   ├── editor/           # 编辑器
│   ├── agent/            # Agent 面板与输入框
│   ├── diff/             # diff 审阅
│   ├── settings/         # 设置（sections/ 下按分区）
│   ├── browser/          # 内嵌浏览器面板
│   └── shared/           # 类型、工具、API 边界
├── src-tauri/src/        # Rust 后端
├── docs-site/            # 本文档站
├── docs/                 # 产品笔记与原型
├── public/               # 静态资源
└── scripts/              # 构建与开发脚本
```

## Rust 模块边界

| 目录 | 职责 |
| --- | --- |
| `commands/` | Tauri IPC 适配，按领域分组（workspace、knowledge、notes、documents、history、sessions、settings、skills、im、agent、builtin_commands、logs、browser） |
| `builtin_commands.rs` | 内置斜杠命令的**唯一注册表** |
| `storage/` | SQLite、FTS、文件扫描与写入、keyring、文档历史、预览抽取 |
| `agent_tools/` | 七个工具的闭集（types / registry / execute） |
| `browser/` | 受控 Chromium 会话（manager / policy / types / fetcher） |
| `runtime/` | Agent 循环、DSML 解析、子 Agent 派生 |
| `domain/` | IPC 与持久化 DTO |
| `skills/` | Skill 目录、安装、执行 |
| `im/` | IM provider 路由 |
| `mcp/` | MCP 客户端、配置、审批、策略 |

### 几条硬约束

- **工具闭集固定为七个**：`search` / `read` / `list` / `edit` / `write` / `run` / `task`。不要加第八个工具名，新能力通过扩展现有工具的参数实现。
- **`task` 是唯一的编排工具**，由 runtime 分发，不走 `AgentTool::execute`。
- **浏览器只能通过 `run` + `kind: "browser"` 访问**，该分支在 `runtime/mod.rs` 里异步预分发。
- **Agent 文件改动必须保持 diff → 确认 → hash 校验 → 原子写入**。

## 前端模块边界

- 功能目录拥有对应界面的 UI；`workspace/` 负责编排快照状态，不引入全局 store。
- 文件 / 会话 / Agent / 审阅动作分别放在 `useEditorActions` / `useKnowledgeBaseActions` / `useSessionActions` / `useAgentTurn` / `useReviewActions`，不要堆进 `WorkspaceShell.tsx`。
- `shared/tauriApi.ts` 是 `shared/api/` 之上的 barrel，也是**唯一的 IPC 边界**。
- 内置命令的 `/` 选择器只负责渲染；命令拦截与分发在 `useAgentTurn.handleSubmitPrompt` 里，且必须在乐观消息持久化**之前**执行。

## 样式

- 设计令牌在 `src/styles/app.css` 的 `:root`，并在 `src/styles/tailwind.css` 里映射。
- 新 UI 使用 Tailwind 工具类 + `cn()`，语义令牌如 `bg-surface`、`text-ink`、`bg-accent`。
- 按钮用 `src/shared/Button.tsx`，对话框用 `ConfirmDialog` / `ModalBackdrop`。
- 不引入 Ant Design、MUI、Bootstrap。

## 下一步

- [文档站维护](/dev/docs-site)
