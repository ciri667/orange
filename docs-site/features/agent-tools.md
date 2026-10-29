# Agent 工具

内置工具为八个：**search / read / list / edit / write / exec / browser / task**。Skill 提供指令与资源，工具执行动作，会话权限决定范围和审批。MCP 是单独启用的外部服务工具。

| 工具 | 工作区内只读 | 工作区内修改，需审批 | 完全权限，需审批 | 完全权限，无需审批 |
| --- | --- | --- | --- | --- |
| search / read / list | 工作区及启用的 Skill 资源 | 同左 | 增加逐次审批的外部读取 | 增加外部读取 |
| edit / write | 禁止 | 生成待审变更 | 生成待审变更 | 自动应用本次新变更 |
| exec / browser | 禁止 | 禁止 | 每次操作审批 | 自动执行 |
| task | 只读角色 | 与父权限取交集 | 与父权限取交集 | 与父权限取交集 |

Plan 覆盖为工作区只读，禁用外部工具、命令及所有变更应用。前两档禁止的动作不能通过一次审批临时放行。

## search / read / list

- search 默认 target=notes，检索会话选中的知识库，支持 query、knowledgeBaseIds、pathPrefix、tags、fileTypes、updatedAfter、updatedBefore；查询参数只能缩小范围。`fileTypes` 支持 markdown、txt、docx、pdf：前两者查笔记 chunk，后两者查文档 chunk 并返回页码/结构块引用。默认返回 4 条引用，最多 16 条。
- 三个工具都支持工作区 path；相对路径以会话所选知识库根目录为基准，根目录用 .，不会随编辑器焦点漂移。
- read 用 fileId 或 path 读取。Markdown/TXT 返回文本；DOCX 返回结构块，PDF 返回按页文本和 `pageCount`。PDF 文本默认按字形坐标重建阅读顺序（≤ 4 MB 且 ≤ 200 页时生效，双栏先左后右）；超出预算或覆盖率不足会回退内容流顺序并在 `warnings` 里说明。普通文件默认 6000 字符，按 nextOffset 续读；PDF 可传 page，或用 page=N&pageEnd=M 读页区间；DOCX/PDF 都可用 block=N 读单个结构块，DOCX 还可用 heading 读标题到下一个同级标题之间的正文。越界页码或结构块会明确失败。DOCX/PDF 的 path 必须对应已扫描且在当前会话授权范围内的文件，否则请使用 fileId。
- read 的 includeImages=auto（默认）会在 PDF 无文本层或指定了 page/block 时返回 documentImages，并在后续图片消息里把页面或 DOCX 内嵌图片交给视觉模型；always 会在有文本层时也渲染最多 4 页（或指定页区间）和 DOCX 图片，适合双栏、扫描或文本顺序错乱的 PDF；none 跳过图片。payload 只含 id、页码、图注和尺寸，不内联 base64。Provider 未打开「支持视觉」时返回 `visionUnavailable` 并说明原因。图片按 8 张/消息、4 MB/张、长边 2048 px 预算处理，长扫描件请按 1–4 页分批读。
- read target=skill、skillId、可选包内 path 读取启用 Skill 的说明、参考、模板和脚本，基础权限即可读取。内置与自定义都读取 `.orange/skills` 中的真实文件，禁止路径或链接逃逸。
- list 默认最多 120 项；可用 query 按文件名/路径子串定位文档，再用 prefix、fileType 缩小范围。
- 完全权限支持工作区外绝对路径；另一个已登记但未选中的知识库按外部路径处理。需审批档每次实际外部读取都审批，不缓存目录授权。
- search target=web 和 read url 属于外部能力，只对两种完全权限档开放，仍需启用联网设置。需审批档每次调用审批。

## edit / write

edit 的 replace 替换唯一片段，可用 occurrence 指定第 N 次；append 追加；multi_replace 用 edits 数组完成多处替换。Markdown/TXT 保留 BOM 和 CRLF。move / rename / delete 管理文本、已支持文档、图片和完整目录；destination 指定移动目标，destinationKnowledgeBaseId 可指定另一授权知识库。

write kind=file 创建 Markdown/TXT，必须提供完整 content 与 targetPath；kind=folder 创建目录。父目录不存在时先单独提出建目录操作。所有新目标禁止覆盖已有文件。

变更进入同一通用变更集，展示文本 diff 或源/目标、影响文件数、警告，可选择独立操作。目录包含隐藏和未索引内容；删除进入系统回收站，失败不降级为永久删除。不能操作知识库根目录、链接逃逸或重叠路径，跨卷移动明确失败并保留源。详见[文件变更与操作审批](/guide/write-safety)。

## exec

参数为 command、可选 cwd 和 timeoutSeconds。默认 cwd 为当前会话第一个知识库根目录，失效时报错。Windows 优先 PowerShell 7，回退系统 PowerShell；macOS/Linux 优先 Bash，回退 /bin/sh。实际 Shell 会在上下文中声明。

每次启动独立非交互进程，不加载交互配置、不连接用户终端、不保留 Shell 状态。先建立 Windows Job Object 或 Unix 进程组再投递命令；停止、超时和结束后清理后代。默认超时 120 秒，可设 5–1800 秒，总捕获输出上限 1 MiB，超出后仍排空管道。

结果区分退出码、stdout、stderr、超时、取消及截断，失败不会作为成功。命令结束后重扫知识库，读取以磁盘为准；编辑器未保存内容继续保留并提示冲突。不会注入 Keyring 密钥，继承环境过滤明显秘密变量；不支持后台服务、密码交互或自动提权。

批准的是命令在真实环境执行；命令内部修改不逐项展示 diff，失败或取消也可能已有副作用。超时和进程清理不构成文件系统沙箱或整机资源配额。

## browser

独立的 browser 工具承载现有 Chromium 操作。浏览器必须显式启用并下载内核，URL 限 http/https。需审批档包括读取在内的每次调用都审批；输入、目标 URL、标签页及已知元素会显示在统一操作审批中。用户接管会取消待审操作并停止当前回合后续工具。截图只给面板和回执，不进入模型；Windows 右侧栏直接嵌入有头 Chromium 原生子窗口，不再传输 JPEG 画面，元素点击等操作也改为单次 JS 求值后走 CDP 输入。页面弹窗（alert / confirm / prompt / beforeunload）会阻塞其它调用，`browser.get_state` 的 `dialogs` 会列出它们，模型用 `page.dialog.accept` / `page.dialog.dismiss` 处理。详见[内嵌浏览器](/features/browser)。

## task

唯一编排工具，内置 explore、researcher 只读，writer 可生成可审阅的文件变更。子任务工具集与父权限取交集，不提供 exec/browser/MCP，不允许递归委派。任务指令须自足。

最多 3 个独立只读任务并行；background 只支持只读任务，完成后通知。续跑绑定父会话、角色和权限版本，收权或跨会话后不能复用旧上下文。

