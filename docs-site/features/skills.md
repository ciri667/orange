# Skills

Skills 是给 Agent 的**工作流说明**：告诉它在某类任务里应该按什么顺序、用什么工具。

## 内置 Skills

| Skill | 名称 | 作用 |
| --- | --- | --- |
| `note-research` | 知识库研究 | 基于已选知识库发现文档、检索和阅读 Markdown，给出带引用的回答 |
| `note-rewrite` | 笔记改写 | 改写指定的 Markdown / TXT，通过待确认 diff 交给你决定是否写入 |
| `draft-from-context` | 上下文草稿 | 基于已选范围创建新的 Markdown 草稿，写入前仍需确认 |
| `organize-knowledge` | 知识整理 | 给出标签、标题、目录、关联笔记建议，**不直接移动或改写文件** |

内置 Skill 是纯指令型工作流，不携带脚本或外部命令。

::: info 内置 Skill 只能启用或禁用
内置 Skill 不能编辑，也不能删除，只能开关。
:::

## 启用与注入

- 在 **设置 → Skills 能力** 里启用 / 禁用 Skill。
- 启用的 Skill 会把「名称 + 描述」注入给 Agent，由 Agent 自主判断是否使用。
- 注入有字符上限（8000 字符），避免装了很多 Skill 后挤占上下文。
- 没有启用任何 Skill 时，Agent 不会收到 Skill 目录。

## 自定义 Skills

在橘记的用户目录下放置自定义 Skill：

```
~/.orange/skills/<skill-name>/SKILL.md
```

`SKILL.md` 是主说明文件，沿用 Codex 的目录化体验。

## 可执行 Skill

需要执行脚本的 Skill 用 `orange-runtime.yaml` 声明入口。执行走 `run` 工具，**可能需要你审批**。

可执行 Skill 需要：

- 会话安全级别为「进阶」及以上。
- 设置里开启了进阶执行。
- 完全级别还需要额外开启自主模式。

::: warning IM 会话不能执行 Skill
`run` 只对本地桌面会话注册，从 IM 发起的会话无法运行 Skill 脚本。
:::

## 下一步

- [Agent 工具](/features/agent-tools)
- [MCP 服务](/features/mcp)
