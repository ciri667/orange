# 文档站维护

本文档站基于 [VitePress](https://vitepress.dev)，源码在仓库的 `docs-site/` 目录，和主项目一起做版本管理。

## 本地预览

```bash
cd docs-site
npm install
npm run dev      # http://localhost:5174/
```

构建检查：

```bash
npm run build
```

## 目录结构

```
docs-site/
├── .vitepress/config.mts   # 站点配置：导航、侧边栏、搜索
├── public/                 # 静态资源（logo 等）
├── index.md                # 首页
├── guide/                  # 开始使用与核心概念
├── features/               # 功能说明
├── reference/              # 设置项、隐私、FAQ
└── dev/                    # 开发者文档
```

## 新增页面

1. 在对应目录新建 `.md` 文件，首行写一级标题：

   ```markdown
   # 页面标题

   正文……
   ```

2. 在 `.vitepress/config.mts` 对应的 `sidebar` 分组里加一条：

   ```ts
   { text: "页面标题", link: "/features/your-page" }
   ```

3. 本地 `npm run dev` 确认渲染正常，再提交。

## 部署

文档站通过 GitHub Actions 部署到 GitHub Pages：

- 工作流：`.github/workflows/docs.yml`
- 触发：推送到 `main` 且改动涉及 `docs-site/**`，或手动触发
- 站点地址：`https://ciri667.github.io/orange/`

::: info `base` 路径
站点挂在 `/orange/` 子路径下，所以 `config.mts` 里的 `base` 必须是 `"/orange/"`。如果以后迁到独立仓库 `orange-docs`，记得同步改成 `"/orange-docs/"`。
:::

## 写作约定

- **内容必须和代码一致**。内置命令表来自 `src-tauri/src/builtin_commands.rs`，工具说明来自 `src-tauri/src/agent_tools/execute.rs`，改了代码要同步改文档。
- 中文文档统一使用 **UTF-8 无 BOM** 编码，避免 Windows 用户看到乱码。
- 用 `::: tip` / `::: warning` / `::: info` / `::: danger` 容器强调重点。
- 截图放在 `docs-site/public/` 下，用绝对路径引用，例如 `/screenshots/quick-start.png`。
