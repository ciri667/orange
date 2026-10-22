import { defineConfig } from "vitepress";

/**
 * 橘记官方文档站配置。
 *
 * 部署位置：GitHub Pages（仓库 ciri667/orange，站点路径 /orange/）。
 * 如果以后换成独立仓库 ciri667/orange-docs，只需把 base 改成 "/orange-docs/"。
 */
export default defineConfig({
  lang: "zh-CN",
  title: "橘记",
  description: "本地优先的 Agent 笔记库，面向个人知识工作。",
  base: "/orange/",

  head: [
    ["link", { rel: "icon", type: "image/png", href: "/orange/logo.png" }],
    ["meta", { name: "theme-color", content: "#f97316" }],
  ],

  // 仓库内图片（docs/imgs、assets/readme）不在 docs-site 下，
  // 这里把仓库根目录加进 public，便于文档直接引用已有截图。
  vite: {
    server: { fs: { allow: [".."] } },
  },

  themeConfig: {
    logo: "/logo.png",

    nav: [
      { text: "指南", link: "/guide/what-is-orange", activeMatch: "/guide/" },
      { text: "功能", link: "/features/knowledge-base", activeMatch: "/features/" },
      { text: "参考", link: "/reference/settings", activeMatch: "/reference/" },
      { text: "开发", link: "/dev/getting-started", activeMatch: "/dev/" },
      {
        text: "GitHub",
        link: "https://github.com/ciri667/orange",
      },
    ],

    sidebar: {
      "/guide/": [
        {
          text: "开始使用",
          items: [
            { text: "橘记是什么", link: "/guide/what-is-orange" },
            { text: "安装", link: "/guide/installation" },
            { text: "五分钟上手", link: "/guide/quick-start" },
          ],
        },
        {
          text: "核心概念",
          items: [
            { text: "知识库", link: "/guide/knowledge-base" },
            { text: "会话与 Agent", link: "/guide/session-and-agent" },
            { text: "写入确认机制", link: "/guide/write-safety" },
          ],
        },
      ],
      "/features/": [
        {
          text: "知识库与检索",
          items: [
            { text: "知识库与文件", link: "/features/knowledge-base" },
            { text: "检索与引用", link: "/features/retrieval" },
            { text: "语义检索", link: "/features/semantic-search" },
          ],
        },
        {
          text: "Agent 能力",
          items: [
            { text: "Agent 工具", link: "/features/agent-tools" },
            { text: "内置命令", link: "/features/builtin-commands" },
            { text: "Skills", link: "/features/skills" },
            { text: "MCP 服务", link: "/features/mcp" },
            { text: "内嵌浏览器", link: "/features/browser" },
            { text: "联网搜索", link: "/features/web-search" },
            { text: "语音输入", link: "/features/voice" },
          ],
        },
        {
          text: "集成",
          items: [{ text: "即时通讯（IM）", link: "/features/im" }],
        },
      ],
      "/reference/": [
        {
          text: "参考",
          items: [
            { text: "设置项总览", link: "/reference/settings" },
            { text: "数据与隐私边界", link: "/reference/privacy" },
            { text: "FAQ 与排错", link: "/reference/faq" },
          ],
        },
      ],
      "/dev/": [
        {
          text: "参与开发",
          items: [
            { text: "开发环境", link: "/dev/getting-started" },
            { text: "项目结构", link: "/dev/project-structure" },
            { text: "文档站维护", link: "/dev/docs-site" },
          ],
        },
      ],
    },

    outline: { level: [2, 3], label: "本页目录" },

    socialLinks: [
      { icon: "github", link: "https://github.com/ciri667/orange" },
    ],

    footer: {
      message: "基于 Apache-2.0 许可发布",
      copyright: "橘记 Orange",
    },

    docFooter: { prev: "上一篇", next: "下一篇" },
    lastUpdated: { text: "最后更新于" },
    returnToTopLabel: "回到顶部",
    sidebarMenuLabel: "目录",
    darkModeSwitchLabel: "外观",
    lightModeSwitchTitle: "切换到浅色模式",
    darkModeSwitchTitle: "切换到深色模式",

    search: {
      provider: "local",
      options: {
        translations: {
          button: { buttonText: "搜索文档", buttonAriaLabel: "搜索文档" },
          modal: {
            noResultsText: "没有找到结果",
            resetButtonTitle: "清除查询",
            footer: {
              selectText: "选择",
              navigateText: "切换",
              closeText: "关闭",
            },
          },
        },
      },
    },
  },
});
