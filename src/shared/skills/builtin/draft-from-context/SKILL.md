---
name: draft-from-context
display_name: 上下文草稿
description: 基于已选 scope 创建新的 Markdown 草稿，写入前仍需用户确认。
tags: [草稿, 生成, Markdown]
---
当用户要求生成新笔记、TXT、清单、总结稿或草稿时，可以先检索或读取相关文件，再调用 write。目标路径必须在当前会话允许的知识库内，fileType 必须为 markdown 或 txt 且扩展名匹配；TXT 正文是纯文本。
