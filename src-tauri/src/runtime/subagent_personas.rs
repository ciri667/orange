//! 子 Agent 角色：内置 explore/researcher/writer，以及 ~/.orange/agents 用户定义。

use crate::domain::WorkspaceSnapshot;
use serde_yaml::Mapping;
use std::fs;
use std::path::{Path, PathBuf};

use super::context::{build_cwd_summary, build_scope_summary};

/** 子级允许的工具名；task 和 run 永远不会进入子注册表。 */
const ALLOWED_CHILD_TOOLS: [&str; 5] = ["search", "read", "list", "edit", "write"];
const DEFAULT_READONLY_TOOLS: [&str; 3] = ["search", "read", "list"];
const WRITER_TOOLS: [&str; 5] = ["search", "read", "list", "edit", "write"];

/** 解析后的子 Agent 角色，内置和用户 markdown 共用此形状。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedAgent {
    pub name: String,
    pub label: String,
    pub description: String,
    pub tools: Vec<String>,
    pub prompt: String,
    pub source: AgentSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentSource {
    Builtin,
    User,
}

impl ResolvedAgent {
    pub(super) fn writable(&self) -> bool {
        self.tools
            .iter()
            .any(|tool| tool == "edit" || tool == "write")
    }

    pub(super) fn visible_tools_line(&self) -> String {
        self.tools.join(", ")
    }
}

/** 按名称解析角色；内置优先，其次用户目录，未知则失败并列出可用名。 */
pub(super) fn resolve_agent(name: &str) -> Result<ResolvedAgent, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(format!("task 需要 agent。可用：{}。", catalog_names()));
    }
    if let Some(builtin) = builtin_agent(trimmed) {
        return Ok(builtin);
    }
    if let Some(custom) = load_user_agents()
        .into_iter()
        .find(|agent| agent.name == trimmed)
    {
        return Ok(custom);
    }
    Err(format!(
        "未知子 Agent 类型：{trimmed}。可用：{}。",
        catalog_names()
    ))
}

/** schema / 错误信息用的角色名列表，内置在前，用户定义追加。 */
pub(super) fn catalog_names() -> String {
    let mut names: Vec<String> = builtin_agents()
        .into_iter()
        .map(|agent| agent.name)
        .collect();
    for agent in load_user_agents() {
        if !names.iter().any(|name| name == &agent.name) {
            names.push(agent.name);
        }
    }
    names.join(", ")
}

fn builtin_agents() -> Vec<ResolvedAgent> {
    ["explore", "researcher", "writer"]
        .into_iter()
        .filter_map(builtin_agent)
        .collect()
}

fn builtin_agent(name: &str) -> Option<ResolvedAgent> {
    match name {
        "explore" => Some(ResolvedAgent {
            name: "explore".to_owned(),
            label: "探索".to_owned(),
            description: "快速定位笔记和目录".to_owned(),
            tools: DEFAULT_READONLY_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
            prompt: "你是探索子 Agent。快速摸清当前知识库里和任务相关的笔记、目录与文档，不要深读无关长文。\
策略：先 list 或 search 定位，再对关键文件做短窗口 read。\
终稿给没读过原文件的父 Agent，按下面结构输出：\n\
## 找到的文件\n- `路径`（id=…）- 一句话说明\n\
## 关键摘录\n只贴真正有用的短片段。\n\
## 结论\n不超过五条。\n\
## 仍缺的信息\n父 Agent 还需要什么。".to_owned(),
            source: AgentSource::Builtin,
        }),
        "researcher" => Some(ResolvedAgent {
            name: "researcher".to_owned(),
            label: "调研".to_owned(),
            description: "多跳阅读并产出交接稿".to_owned(),
            tools: DEFAULT_READONLY_TOOLS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
            prompt: "你是调研子 Agent。对任务做多跳阅读和交叉引用，产出一份可交接的调研稿。\
策略：search 找入口，read 跟进关键笔记，必要时再 search 验证。不要满足于标题命中。\
终稿给没读过原文件的父 Agent，按下面结构输出：\n\
## 问题\n一句话。\n\
## 证据\n每条证据带来源路径或笔记 id，以及短摘录。\n\
## 结论\n综合判断。\n\
## 开放问题\n仍不确定或知识库里没有的部分。".to_owned(),
            source: AgentSource::Builtin,
        }),
        "writer" => Some(ResolvedAgent {
            name: "writer".to_owned(),
            label: "写作".to_owned(),
            description: "起草待确认改写或新建".to_owned(),
            tools: WRITER_TOOLS.iter().map(|tool| (*tool).to_owned()).collect(),
            prompt: "你是写作子 Agent。根据任务起草 Markdown/TXT 改写或新建，只能通过 edit / write 生成待确认 diff，不能声称已经写入磁盘。\
不要调用 task 或 run。改写前先 read 目标。局部替换用 operation=replace，文末追加用 append，多处编辑用 multi_replace。\
终稿说明改了哪些文件、为什么，以及还需要父 Agent 或用户确认的事项。".to_owned(),
            source: AgentSource::Builtin,
        }),
        _ => None,
    }
}

/** 读取用户目录 orange/agents 下的 markdown；坏文件跳过，不阻断内置角色。项目级目录默认不加载。 */
pub(super) fn load_user_agents() -> Vec<ResolvedAgent> {
    let Some(dir) = user_agents_dir() else {
        return Vec::new();
    };
    load_agents_from_dir(&dir)
}

fn user_agents_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".orange").join("agents"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub(super) fn load_agents_from_dir(dir: &Path) -> Vec<ResolvedAgent> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        match parse_agent_markdown(&content) {
            Ok(agent) if builtin_agent(&agent.name).is_none() => {
                if !agents
                    .iter()
                    .any(|existing: &ResolvedAgent| existing.name == agent.name)
                {
                    agents.push(agent);
                }
            }
            _ => {}
        }
    }
    agents.sort_by(|left, right| left.name.cmp(&right.name));
    agents
}

/** 解析角色 markdown：YAML frontmatter 的 name/description/tools + 正文为 system。 */
pub(super) fn parse_agent_markdown(content: &str) -> Result<ResolvedAgent, String> {
    let normalized = content.strip_prefix('\u{feff}').unwrap_or(content);
    if !normalized.starts_with("---") {
        return Err("缺少 YAML frontmatter。".to_owned());
    }
    let mut lines = normalized.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err("frontmatter 起始标记必须是 ---。".to_owned());
    }
    let mut frontmatter_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter && line.trim() == "---" {
            in_frontmatter = false;
            continue;
        }
        if in_frontmatter {
            frontmatter_lines.push(line);
        } else {
            body_lines.push(line);
        }
    }
    if in_frontmatter {
        return Err("frontmatter 缺少结束标记 ---。".to_owned());
    }
    let mapping = serde_yaml::from_str::<Mapping>(&frontmatter_lines.join("\n"))
        .map_err(|error| format!("frontmatter 不是有效 YAML：{error}"))?;
    let name = yaml_string(&mapping, "name").ok_or_else(|| "frontmatter 缺少 name。".to_owned())?;
    let description = yaml_string(&mapping, "description")
        .ok_or_else(|| "frontmatter 缺少 description。".to_owned())?;
    let label = yaml_string(&mapping, "label").unwrap_or_else(|| name.clone());
    let tools = sanitize_child_tools(yaml_string_list(&mapping, "tools"));
    let prompt = body_lines.join("\n").trim().to_owned();
    if prompt.is_empty() {
        return Err("角色正文不能为空。".to_owned());
    }
    Ok(ResolvedAgent {
        name,
        label,
        description,
        tools,
        prompt,
        source: AgentSource::User,
    })
}

fn sanitize_child_tools(raw: Vec<String>) -> Vec<String> {
    let mut tools = Vec::new();
    for item in raw {
        let name = item.trim().to_ascii_lowercase();
        if ALLOWED_CHILD_TOOLS.contains(&name.as_str())
            && !tools.iter().any(|existing| existing == &name)
        {
            tools.push(name);
        }
    }
    if tools.is_empty() {
        DEFAULT_READONLY_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect()
    } else {
        tools
    }
}

fn yaml_string(mapping: &Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_owned()))
        .and_then(|value| match value {
            serde_yaml::Value::String(text) => Some(text.trim().to_owned()),
            serde_yaml::Value::Number(number) => Some(number.to_string()),
            serde_yaml::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
}

fn yaml_string_list(mapping: &Mapping, key: &str) -> Vec<String> {
    let Some(value) = mapping.get(serde_yaml::Value::String(key.to_owned())) else {
        return Vec::new();
    };
    match value {
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(text) => Some(text.trim().to_owned()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
            .collect(),
        serde_yaml::Value::String(text) => text
            .split(',')
            .map(|part| part.trim().trim_matches('"').trim_matches('\'').to_owned())
            .filter(|value| !value.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/** 子 Agent 的 system prompt：基线范围 + 角色 + 委派声明。不注入父级对话。 */
pub(super) fn build_subagent_system_prompt(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    agent: &ResolvedAgent,
) -> String {
    let session = &snapshot.sessions[session_index];
    let scope_summary = build_scope_summary(snapshot, session);
    let cwd_summary = build_cwd_summary(snapshot, session);
    let tools = agent.visible_tools_line();
    let write_clause = if agent.writable() {
        "你可以调用 edit / write 生成待确认 diff，不能声称已经写入文件。不要调用 task 或 run。"
    } else {
        "只能 search / read / list。不要尝试 edit、write、run 或再次委派。"
    };
    format!(
        "你是橘记的本地优先知识库 Agent，当前以子 Agent 身份运行。当前可见工具：{tools}。\n\
你是被委派的子 Agent。权限在启动时已钉死，不能扩大。{write_clause}任务需要超出范围的能力时，在终稿里写明限制，交给父 Agent 处理。你看不到父级对话，只根据本条任务工作。终稿给没读过原文件的父 Agent 用：列文件、摘关键段落、给结论，不要把探索过程原样倒回去。引用只允许来自已执行工具结果。必须使用服务端标准 tool_calls 字段调用工具。\n\
search 只检索 Markdown；read 可作用于当前 scope 内的 Markdown/TXT，省略 fileId 时读取当前激活文件；DOCX/PDF 用 read 只读抽取。需要看目录时使用 list。TXT 必须原样按纯文本处理。\n\
{}\n\n【范围】\n允许 scope：{scope_summary}\n{cwd_summary}",
        agent.prompt
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_markdown_reads_tools_array_and_csv() {
        let parsed = parse_agent_markdown(
            "---\nname: scout\ndescription: Fast recon\ntools: [search, read, list, task, run]\n---\n\nGo scout.\n",
        )
        .unwrap();
        assert_eq!(parsed.name, "scout");
        assert_eq!(parsed.tools, vec!["search", "read", "list"]);
        assert!(!parsed.writable());
        assert_eq!(parsed.prompt, "Go scout.");
    }

    #[test]
    fn parse_agent_markdown_defaults_tools_and_strips_unknown() {
        let parsed = parse_agent_markdown(
            "---\nname: notes\ndescription: Notes only\ntools: bash, grep\n---\n\nRead notes.\n",
        )
        .unwrap();
        assert_eq!(parsed.tools, vec!["search", "read", "list"]);
    }

    #[test]
    fn writer_builtin_is_writable() {
        let writer = resolve_agent("writer").unwrap();
        assert!(writer.writable());
        assert!(writer.tools.iter().any(|tool| tool == "edit"));
    }

    #[test]
    fn builtin_names_win_over_unknown() {
        assert!(resolve_agent("explore").is_ok());
        assert!(resolve_agent("missing-agent")
            .unwrap_err()
            .contains("explore"));
    }
}
