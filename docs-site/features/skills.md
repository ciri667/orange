# Skills

Skill 提供任务指令与资源；Tool 提供动作；会话权限决定动作是否允许以及如何审批。Skill 自身没有“可执行权限等级”。

## 内置与自定义

| Skill | 用途 |
| --- | --- |
| note-research | 检索、阅读知识库并给出引用 |
| note-rewrite | 为指定 Markdown/TXT 生成文件变更 |
| draft-from-context | 根据已选范围提出新草稿 |
| organize-knowledge | 组织标题、标签、目录与关联建议 |

内置 Skill 与自定义 Skill 使用同一套目录结构。应用首次启动时会把随包发布的 `SKILL.md` 同步到 `~/.orange/skills/builtin/`；自定义 Skill 直接安装到 `~/.orange/skills/<skill>/`。运行时只从这个目录读取，SQLite 只保存启用状态等覆盖信息。参考、模板和脚本随包保存，安装只复制资源，不触发脚本。

## 渐进加载

1. 初始上下文仅提供已启用 Skill 的名称、描述和 skill:资源标识，不注入所有正文。
2. 模型按需用 read target=skill、skillId 加载主指令，指定 path 读取 references、模板或 scripts 内的文本资源。
3. 用户显式选择与模型主动读取使用相同解析、来源标记和权限校验。内置 Skill 也通过 `.orange/skills` 中的真实 `SKILL.md` 读取。
4. 基础只读权限可读取启用包的资源；相对路径逃逸、符号链接和 Windows 目录联接点均拒绝。读取脚本不代表允许执行。
5. 同回合重复资源按版本去重，压缩后恢复已加载指令；禁用或更新后不再复用旧内容。

## 脚本与历史包

脚本通过通用 exec 执行，无需专用运行清单。只有两种完全权限档可使用 exec；需审批档每次展示准确命令、cwd 与超时，无需审批档自动执行。Plan、IM、定时任务及子 Agent 不开放 exec。

Skill 没有独立运行时或执行权限。脚本只能由通用 exec 工具按当前会话权限执行，读取脚本也不会授予执行权限；文件修改统一进入通用变更集审阅流程。

- [Agent 工具](/features/agent-tools)
- [文件变更与操作审批](/guide/write-safety)
