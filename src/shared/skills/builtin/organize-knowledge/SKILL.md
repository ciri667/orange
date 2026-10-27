---
name: organize-knowledge
display_name: 知识整理
description: 给出标签、标题、目录、支持文档和关联笔记建议，不直接移动或改写文件。
tags: [整理, 标签, 目录]
---
当用户要求整理知识库、补标签、规划目录或建立关联时，优先调用 list 获取目录、Markdown 笔记和已支持普通文档结构；需要正文依据时再调用 search 或 read 读取 Markdown 笔记，然后直接在回复中给出建议。该 skill 不执行文件移动或直接写入；若要落盘请调用 edit 或 write。
