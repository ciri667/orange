# 橘记（Orange）

橘记是一款面向个人知识工作的本地优先 Agent 笔记应用。它将本地 Markdown 文件夹视为用户拥有的知识库，并在熟悉的桌面笔记工作台中，让助手完成查找、问答、改写、生成草稿和整理知识等操作。

产品围绕清晰的数据边界设计：笔记始终保留为用户选择的本地目录中的 Markdown 文件；Agent 只基于用户选择的上下文工作；任何写入操作都必须先展示 diff，并在用户确认后才修改笔记内容。

## 产品原则

- 本地优先的知识所有权：Markdown 文件是主要数据源。
- 明确的上下文控制：每个 Agent 会话都有独立的知识库范围。
- 写入必须确认：Agent 编辑先作为建议提出，确认后才应用。
- 回答可追溯来源：知识库问答包含笔记标题、路径和命中片段。
- 桌面效率工具布局：文件导航、编辑器和 Agent 侧栏同时可见。

## 核心体验

橘记使用三栏工作台：

- 左侧：知识库切换、本地目录树和搜索。
- 中间：Markdown 编辑器、笔记元信息、新建笔记和 Agent diff 预览。
- 右侧：Agent 会话、已选知识库范围、快捷操作、引用来源和输入框。

Agent 侧栏以会话为核心。一个会话会绑定消息、已选择的知识库、当前笔记、锁定笔记和待确认写入建议，避免无关问题、编辑和引用混在同一段上下文中。

知识库范围按会话选择。当前激活知识库默认选中，用户可以为该会话显式加入更多知识库。

## Agent 能力

- `ask`：基于已选择的知识库范围回答问题。
- `find`：查找相关笔记并返回可追溯引用。
- `rewrite`：为当前笔记提出改写建议，并在写入前展示 diff。
- `create`：在当前激活知识库中生成新的 Markdown 草稿。
- `organize`：建议标签、标题、目录位置或关联笔记。

## 数据模型

主要产品对象包括：

- `KnowledgeBase`：用户选择的本地 Markdown 目录。
- `Note`：包含标题、路径、标签、反向链接和正文内容的 Markdown 文件。
- `AgentSession`：包含消息、已选知识库、当前笔记、锁定笔记和待确认变更的上下文容器。
- `AgentMessage`：用户或助手消息，可包含操作类型和引用来源。
- `ProposedChange`：Agent 提出的待确认写入操作。
- `Citation`：包含知识库、笔记、路径和命中片段的来源引用。

## 技术实现

当前仓库已经从可运行前端原型切换为正式开发结构：

- 前端使用 Vite、React 和 TypeScript，按 `workspace`、`knowledge-base`、`editor`、`agent`、`diff`、`settings`、`shared` 拆分模块。
- 桌面端使用 Tauri v2 和 Rust，`src-tauri/` 负责本地目录选择、Markdown 扫描、SQLite/FTS5 索引、Agent Runtime 和安全写入确认。
- Agent 以 OpenAI-compatible格式 loop 方式运行，检索被建模为 `search_notes` 工具；模型未启用、密钥缺失或请求失败时会回退到本地规则 Agent。
- 写入类能力只能生成 `ProposedChange`，确认后才由本地层执行路径校验、hash 校验和原子写入。

旧版前端原型已归档到 `docs/prototype/`，用于核对产品行为和 UI 交互，不再作为正式运行入口。

## 项目结构

```text
src/
  agent/          Agent 会话面板、工具调用轨迹和引用展示
  diff/           Agent 写入确认面板
  editor/         Markdown 编辑器区域
  knowledge-base/ 知识库切换、目录树和搜索
  settings/       知识库、模型和写入策略设置
  shared/         前后端共享类型、Tauri API 适配和浏览器 mock runtime
  workspace/      三栏工作台状态编排
src-tauri/
  src/            Tauri commands、Agent runtime、SQLite/FTS5 索引和安全写入
docs/
  product/        产品原型说明
  prototype/      已归档的旧版前端原型
```

## 开发命令

安装依赖：

```bash
npm install
```

启动本地开发服务：

```bash
npm run dev
```

构建并执行类型检查：

```bash
npm run build
```

启动桌面开发服务：

```bash
npm run desktop:dev
```

执行 Rust 测试：

```bash
npm run rust:test
```

构建 macOS 桌面包：

```bash
npm run desktop:build
```

使用 Vite 开发服务时，应用通常运行在 `http://localhost:5173/`。

## 产品文档

详细产品原型说明见 [docs/product/product-prototype.md](docs/product/product-prototype.md)。修改界面或接入真实本地文件与 Agent 能力时，可以用它核对交互、数据边界和产品行为。
