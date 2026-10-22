# 开发环境

## 前置条件

- Node.js 20 或更高版本
- Rust stable 与当前平台的 Tauri 构建依赖
- **Windows 11**：安装 Visual Studio 2022 的「使用 C++ 的桌面开发」工作负载和 Microsoft Edge WebView2，并使用 `stable-msvc` Rust 工具链
- **仅在构建 IM 网关时**：Go 1.22 或更高版本

## 安装依赖

```bash
npm install
```

## 常用命令

| 命令 | 作用 |
| --- | --- |
| `npm run dev` | 启动 Vite 前端开发服务（通常 `http://localhost:5173/`） |
| `npm run build` | TypeScript 类型检查 + 前端生产构建 |
| `npm run preview` | 本地预览生产构建 |
| `npm run desktop:dev` | 启动完整 Tauri 桌面开发环境 |
| `npm run desktop:build` | 构建本机验收用的包（ad-hoc 签名，无证书） |
| `npm run desktop:build:dmg` | 构建本地安装流程验收用的 DMG |
| `npm run desktop:build:release` | 构建正式发布包（需要 `ORANGE_RELEASE_SIGNING_IDENTITY`） |
| `npm run rust:test` | 运行 Rust 测试 |
| `npm run icon:macos:generate` | 从插画重新生成 macOS 图标 |

## Windows 环境确认

首次安装 Rust 后需要重启终端或 IDE，让 `cargo` 加入 `PATH`：

```powershell
rustup default stable-msvc
npx tauri info
```

## 构建 IM 网关 sidecar

IM 网关由独立 sidecar 提供。构建所有已注册 provider：

```bash
npm run sidecar:im:build
```

也可以按平台单独构建：

```bash
npm run sidecar:feishu:build
npm run sidecar:qq:build
npm run sidecar:weixin:build
npm run sidecar:wecom:build
```

产物生成到 `src-tauri/sidecars/bin/`，该目录不提交到仓库。**首次使用 IM 网关或打包前，必须先完成构建。**

## 提交前检查

- 前端改动：至少跑 `npm run build`。
- 涉及 Rust 或文件系统行为：同时跑 `npm run rust:test`。
- 涉及可见 UI：在 `npm run dev` 和 `npm run desktop:dev` 下都手动验证一遍。

## 下一步

- [项目结构](/dev/project-structure)
- [文档站维护](/dev/docs-site)
