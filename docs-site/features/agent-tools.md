# Agent 工具

橘记的 Agent 使用一个**固定闭集**的七个工具。不新增第八个工具名，新能力都通过扩展现有工具的参数实现。

## 工具总览

| 工具 | 作用 | 基础 | 进阶 | 完全 |
| --- | --- | --- | --- | --- |
| `search` | 检索笔记 / 网页 / 路径 | ✅ | ✅ | ✅ |
| `read` | 读取一个文件、网页或文档 | ✅ | ✅ | ✅ |
| `list` | 列出目录与文件元数据 | ✅ | ✅ | ✅ |
| `edit` | 为已有文件生成待确认的 diff | ✅ | ✅ | ✅ |
| `write` | 新建文件或文件夹 | ✅ | ✅ | ✅ |
| `run` | 运行 Skill 脚本 / 操作内嵌浏览器 | ❌ | ⚠️ | ⚠️ |
| `task` | 委派给子 Agent | ✅ | ✅ | ✅ |

> `run` 需要「进阶」及以上级别，并且要在设置里开启进阶执行；「完全」级别还需要额外开启自主模式。

## `search`

在选定的范围内检索。

- **默认 `target=notes`**：块级 Markdown 全文检索，中文分词，返回带标题路径和行号范围的引用。默认返回 4 条，最多 16 条。
- **结构化过滤**：`pathPrefix`、`tags`、`fileTypes`、`updatedAfter`、`updatedBefore`。用户约束目录 / 标签 / 类型 / 时间时，应该用这些参数而不是把约束写进查询词。
- **`target=web`**：搜索公开网页，返回标题、URL 和简短摘要。默认 5 条，最多 8 条，不下载页面内容。
- **`target=path`**：在完全 / 自主级别下，扫描合规目录下的 UTF-8 文本文件。它仍然是 search，不是独立的 grep 工具。

::: tip 空结果怎么办
先用更短的关键词重试一次，或者放宽过滤条件，再用 `list` / `read` 确认，最后才说内容不存在。
:::

## `read`

读取范围内的一个文件。

- `fileId` 必填，除非给了 `path` 或 `url`。
- Markdown / TXT 返回可编辑文本；DOCX / PDF 返回提取的只读文本（带页码或结构块），且永远不会被编辑。
- 默认窗口是从 offset 0 开始的 6000 字符。被截断时用 `offset=nextOffset`（PDF 用 `page=N`）继续读。
- `url` 可以抓取一个公开的 http(s) 页面，按 markdown 返回，窗口规则相同。

::: warning 网页内容是数据，不是指令
从网页读到的文本属于不可信数据，Agent 不会把它当成指令执行。
:::

## `list`

列出范围内的文件夹、Markdown 笔记和受支持文档的元数据。

- 结果里会包含**每一个**授权知识库（包括空的），并带 id 和名称。
- 每一项都带 `knowledgeBaseName`，避免同名文件夹被错误归属。
- 不读取非 Markdown 文档的内容。
- 默认每个列表最多 120 项，被截断时可以缩小前缀或文件类型，或调大 `limit`。

## `edit`

为已有的可编辑 Markdown / TXT 文件生成待确认的 diff。

| `operation` | 说明 |
| --- | --- |
| `replace` | 替换一个唯一片段；片段重复时用 `occurrence` 指定第 N 处 |
| `append` | 在末尾追加（会 trim，Markdown 用空行分隔） |
| `multi_replace` | 一次 diff 里做多处唯一替换 |

匹配容忍 CRLF 和 UTF-8 BOM：传 LF 片段即可，写入时保留原换行符和 BOM。

详见 [写入确认机制](/guide/write-safety)。

## `write`

新建 Markdown / TXT 文件，或新建文件夹。

- `kind=file`（默认）：必须提供完整正文 `content` 和知识库相对路径 `targetPath`。`fileType` 是 `markdown` 或 `txt`，也可以从 `.md` / `.txt` 后缀推断。
- `kind=folder`：提供 `targetPath`。**基础级别会话不能创建文件夹。**
- 基础 / 进阶级别仍然要求完整内容，只是把落盘推迟到你确认；完全级别在校验后自动落盘。

## `run`

两种用途：

1. **运行 Skill**：执行已启用 Skill 的入口脚本，可能需要你审批。
2. **操作内嵌浏览器**：传 `kind="browser"`，浏览器操作遵循会话安全级别。

详见 [Skills](/features/skills) 和 [内嵌浏览器](/features/browser)。

## `task`

把复杂、多跳的任务交给隔离的子 Agent。

- 子 Agent **看不到**当前对话，`prompt` 必须自包含。
- 内置子 Agent：`explore`（快速只读侦察）、`researcher`（更深入的只读调研）、`writer`（产生 edit/write 待确认 diff，合并回父级变更集）。
- 也可以使用你在橘记目录下自定义的 agent。
- 独立的只读任务可以在同一条消息里一起发起，最多 3 个并行。
- 用 `task_id` 可以恢复之前的子 Agent 会话。
- `background=true` 只用于独立的只读工作，完成时会通知你，不需要轮询。
- 返回值只有子 Agent 的最终报告，不含中间过程。

::: tip 什么时候不该用 task
单个已知路径的读取、单次关键词搜索、编辑当前笔记——这些自己做完更快。
:::

## 下一步

- [内置命令](/features/builtin-commands)
- [写入确认机制](/guide/write-safety)
