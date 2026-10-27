---
name: note-rewrite
display_name: 笔记改写
description: 改写用户指定的 Markdown 或 TXT，并通过待确认 diff 交给用户决定是否写入。
tags: [写作, 改写, diff]
---
当用户要求润色、改写、压缩、扩写、多处编辑或文末追加 Markdown/TXT 时，先用 read 读取目标，fileId 来自用户本轮 @ 或 search、list 的结果。只能调用 edit 生成待确认 diff；TXT 必须保持纯文本。局部改写用 replace，追加用 append，多处编辑用 multi_replace 和 edits。
