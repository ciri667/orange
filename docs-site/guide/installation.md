# 安装

## 系统要求

| 平台 | 要求 |
| --- | --- |
| macOS | 支持当前主流的 macOS 版本 |
| Windows | Windows 11，需要安装 Microsoft Edge WebView2（Windows 11 通常已自带） |

## 从源码构建

橘记目前以源码构建为主。请先准备好 [开发环境](/dev/getting-started)，然后：

```bash
npm install
npm run desktop:build
```

构建产物位于 `src-tauri/target/release/bundle/` 下。

### macOS 打包说明

| 命令 | 产物 | 用途 |
| --- | --- | --- |
| `npm run desktop:build` | 免证书 ad-hoc 签名的 `.app` | 本机验收，可直接运行 |
| `npm run desktop:build:dmg` | 带 `Applications` 快捷方式的本地测试 DMG | 验证安装流程 |
| `npm run desktop:build:release` | 正式签名的 `.app` 和 `.dmg` | 对外分发，需要 `ORANGE_RELEASE_SIGNING_IDENTITY` |

::: warning 前两个命令不能用于对外分发
`desktop:build` 和 `desktop:build:dmg` 不读取也不创建钥匙串证书，只用于本机验证。对外发布必须使用 `desktop:build:release`，并完成 Apple 签名和公证。
:::

### Windows 打包说明

在 Windows 上，`npm run desktop:build` 会自动使用 `src-tauri/tauri.windows.conf.json` 构建 NSIS 安装包，不需要 macOS 签名配置。

## 开发模式运行

```bash
npm run desktop:dev
```

这会同时启动 Vite 开发服务和 Tauri 桌面窗口，适合边改代码边看效果。

## 首次启动之后

安装完成后，按 [快速上手](/guide/quick-start) 完成三步配置即可开始使用。
