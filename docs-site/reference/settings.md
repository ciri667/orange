# 设置项总览

设置面板分为「配置」和「诊断」两组。

配置开关和选项在操作后自动保存，需要确认的修改在确认后保存。文字配置在输入框失焦、按回车、切换设置分区或关闭面板时保存；面板顶部显示保存状态，失败时可修正或重试。API Key 等密钥仍通过「保存密钥」单独写入系统安全存储。

## 配置

| 设置项 | 说明 |
| --- | --- |
| [知识库管理](/guide/knowledge-base) | 添加 / 移除 / 重扫知识库，打开项目说明文件，管理知识库记忆 |
| [模型设置](/guide/quick-start#第-2-步-配置模型) | 添加并启用模型服务，管理 API Key 与可用模型 |
| [语音输入](/features/voice) | 下载 / 删除本机中文语音模型 |
| [语义检索](/features/semantic-search) | 本地或云端 embedding 与混合检索 |
| [联网搜索](/features/web-search) | 搜索供应商与密钥 |
| [内嵌浏览器](/features/browser) | 启用受控 Chromium，下载浏览器内核；可设置网页链接默认在内置浏览器打开 |
| [Agent 权限](/guide/session-and-agent#安全级别) | 本地新会话默认四档权限与命令超时；旧执行开关不再授权 |
| [即时通讯](/features/im) | IM provider 配置、白名单、默认知识库范围 |
| [Skills 能力](/features/skills) | 启用 / 禁用内置与自定义 Skill |
| [MCP 服务](/features/mcp) | 外部 MCP 工具服务 |
| [Agent 记忆](/guide/session-and-agent#记忆) | 按知识库维度的会话记忆 |

## 诊断

| 设置项 | 说明 |
| --- | --- |
| 运行日志 | 分级日志，按知识库查看 |
| 请求审计 | 本地审计记录，可回看每次 Agent 请求 |

## 模型接入类型

| 类型 | 需要填写 |
| --- | --- |
| OpenAI 兼容接口 | Base URL、API Key、模型列表 |
| 本地 Ollama | 服务地址 |
| 本地 LM Studio | 服务地址 |

## 下一步

- [数据与隐私边界](/reference/privacy)
- [FAQ 与排错](/reference/faq)
