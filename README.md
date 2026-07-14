# 橘记（Orange）

橘记是一款面向个人知识工作的本地优先 Agent 笔记应用。它将本地 Markdown 文件夹视为用户拥有的知识库，并在熟悉的桌面笔记工作台中，让助手完成查找、问答、改写、生成草稿和整理知识等操作。

产品围绕清晰的数据边界设计：笔记始终保留为用户选择的本地目录中的 Markdown 文件；Agent 只基于用户选择的上下文工作；任何写入操作都必须先展示 diff，并在用户确认后才修改笔记内容。

## 项目结构

```text
src/
  agent/          Agent 会话面板、工具调用轨迹和引用展示
  diff/           Agent 写入确认面板
  editor/         Markdown 编辑器区域
  knowledge-base/ 知识库切换、目录树和搜索
  settings/       知识库、模型、写入策略和即时通讯设置
  shared/         前后端共享类型、Tauri API 适配和浏览器 mock runtime
  workspace/      三栏工作台状态编排
src-tauri/
  src/            Tauri commands、Agent runtime、SQLite/FTS5 索引和安全写入
  im/             IM provider 路由与各平台网关接入（飞书等）
  sidecars/       旁路 sidecar 源码与构建产物，由独立语言工具链生成
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

构建 IM sidecar（飞书等长连接网关，依赖本地 Go 工具链）：

```bash
# 构建所有已注册 provider 的 sidecar
npm run sidecar:im:build

# 只构建飞书 provider，桌面集成开发时常用
npm run sidecar:im:build -- --provider feishu
```

首次运行桌面端或打包前需要构建所需 sidecar，否则网关启动会因找不到可执行文件而失败。产物输出到 `src-tauri/sidecars/bin/`，开发态从该目录加载，打包态从 Tauri 资源目录加载。

### 飞书审批卡片配置

待确认笔记改动会以飞书 Card 2.0 审批卡片发送，支持“详情 / 确认写入 / 取消”。在飞书开发者后台需完成以下配置后再启动橘记网关：

- 事件订阅使用长连接，并订阅 `im.message.receive_v1`。
- 在回调配置中启用 `card.action.trigger`，否则按钮不会触发回调。
- 为应用授予发送和接收 IM 消息所需权限，并发布最新应用版本。

卡片不可用时，消息中的“详情 / 确认 / 取消 <编号>”文字指令仍可作为降级方式。群聊中只有发起该变更的用户可以确认或取消。

构建 macOS 桌面包：

```bash
npm run desktop:build
```

使用 Vite 开发服务时，应用通常运行在 `http://localhost:5173/`。
