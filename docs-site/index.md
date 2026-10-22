---
layout: home

hero:
  name: 橘记
  text: 本地优先的 Agent 笔记库
  tagline: 你的笔记还是你的本地文件；Agent 只在授权的知识库里工作，每次写入都先给你看 diff。
  image:
    src: /logo.png
    alt: 橘记
  actions:
    - theme: brand
      text: 快速上手
      link: /guide/quick-start
    - theme: alt
      text: 橘记是什么
      link: /guide/what-is-orange
    - theme: alt
      text: GitHub
      link: https://github.com/ciri667/orange

features:
  - icon: 📁
    title: 笔记就是本地文件
    details: 选一个本地目录作为知识库，笔记仍是普通的 Markdown / TXT 文件。移除知识库授权不会删除原文件，随时可以用任何编辑器打开。
  - icon: 🔍
    title: 标题级检索与可验证引用
    details: 按多级标题拆块、中文分词建索引，检索结果带「标题路径 · 行号范围」定位，回答里的引用能点回原文核对。
  - icon: 🛡️
    title: 写入先看 diff
    details: Agent 的任何文件改动都先生成可审阅的 diff，经路径校验、内容 hash 校验和你的确认后才原子写入。
  - icon: 🧰
    title: 七个工具的闭集
    details: search / read / list / edit / write / run / task 构成固定的工具集，按会话权限动态注册，高权限工具对模型直接不可见。
  - icon: 💬
    title: 从 IM 里用
    details: 可选接入飞书 / Lark、QQ、个人微信、企业微信，在聊天里发起检索或写入请求，写入变更仍需审批。
  - icon: 🔒
    title: 密钥进系统钥匙串
    details: 模型 API Key 存入 macOS Keychain / Windows 凭据管理器，界面和日志都不回显明文。
---
