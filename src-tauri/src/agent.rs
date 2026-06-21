use crate::domain::{
    AgentMessage, AgentToolCall, AgentTurnRequest, AgentTurnResult, Citation, ProposedChange,
    WorkspaceSnapshot,
};
use crate::storage::{create_id, hash_content};
use serde_json::json;
use std::collections::HashSet;
use tauri::AppHandle;

/** 执行 Agent 单轮 loop，核心规则是由 Agent 决定是否调用检索工具。 */
pub fn run_agent_turn(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
) -> AgentTurnResult {
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == request.session_id)
        .unwrap_or(0);
    let active_note = snapshot
        .notes
        .iter()
        .find(|note| note.id == request.active_note_id)
        .cloned();
    let user_message = AgentMessage {
        id: create_id("user"),
        role: "user".to_owned(),
        content: request.prompt.clone(),
        action: Some(request.action.clone()),
        citations: None,
        tool_calls: None,
    };
    let mut tool_calls = Vec::new();
    let mut citations = Vec::new();
    let content;

    snapshot.sessions[session_index].messages.push(user_message);

    match request.action.as_str() {
        "rewrite" => {
            content = propose_rewrite(&mut snapshot, session_index, active_note, &mut tool_calls);
        }
        "create" => {
            content = propose_create(&mut snapshot, session_index, &request, &mut tool_calls);
        }
        "organize" => {
            tool_calls.push(tool_call(
                "suggest_organization",
                "已生成整理建议；该工具不会直接写入文件",
                json!({ "noteId": request.active_note_id }),
            ));
            content =
                "建议先保持当前目录不变，并补充稳定标签、反向链接和来源说明。该建议不涉及写入。"
                    .to_owned();
        }
        _ if should_use_search_tool(&request.prompt, &request.action) => {
            match crate::storage::search_notes(
                app,
                &snapshot,
                &snapshot.sessions[session_index].knowledge_base_ids,
                &request.prompt,
            ) {
                Ok(indexed_citations) => {
                    citations = indexed_citations;
                    tool_calls.push(tool_call(
                        "search_notes",
                        &format!("在会话允许范围内检索到 {} 条候选引用", citations.len()),
                        json!({
                            "query": request.prompt.clone(),
                            "scope": snapshot.sessions[session_index].knowledge_base_ids.clone()
                        }),
                    ));
                }
                Err(error) => {
                    citations = search_snapshot_notes(&snapshot, session_index, &request.prompt);
                    tool_calls.push(tool_call_with_status(
                        "search_notes",
                        &format!("FTS5 检索失败，已降级为当前快照检索：{error}"),
                        json!({
                            "query": request.prompt.clone(),
                            "scope": snapshot.sessions[session_index].knowledge_base_ids.clone()
                        }),
                        "failed",
                    ));
                }
            }

            if let Some(first_citation) = citations.first() {
                tool_calls.push(tool_call(
                    "read_note",
                    &format!("已读取最相关笔记《{}》用于组织回答", first_citation.title),
                    json!({ "noteId": first_citation.note_id }),
                ));
            }

            content = if citations.is_empty() {
                "我调用了检索工具，但在当前会话允许的知识库范围内没有找到足够相关的内容。"
                    .to_owned()
            } else {
                "我调用了检索工具，并只基于当前会话允许的知识库范围组织回答。本地优先的关键是 Markdown 文件归用户所有，索引和模型请求只是辅助层，写入必须先形成 diff。".to_owned()
            };
        }
        _ => {
            content = "这轮问题不需要访问本地知识库。我会先作为知识库助手给出通用建议；需要依据笔记时会显式调用工具。".to_owned();
        }
    }

    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content,
            action: Some(request.action),
            citations: Some(citations),
            tool_calls: Some(tool_calls),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

    AgentTurnResult { snapshot }
}

/** 判断本轮 Agent 是否需要调用检索工具。 */
fn should_use_search_tool(prompt: &str, action: &str) -> bool {
    let normalized_prompt = prompt.to_lowercase();
    let intent_words = [
        "查找",
        "搜索",
        "引用",
        "来源",
        "知识库",
        "笔记",
        "文档",
        "资料",
        "总结",
        "关于",
        "为什么",
        "哪些",
        "markdown",
        "rag",
        "检索",
    ];

    action == "find"
        || intent_words
            .iter()
            .any(|word| normalized_prompt.contains(word))
}

/** 根据会话范围执行快照级检索，作为索引检索失败时的降级恢复。 */
fn search_snapshot_notes(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    prompt: &str,
) -> Vec<Citation> {
    let selected_ids: HashSet<&str> = snapshot.sessions[session_index]
        .knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect();
    let prompt_terms: Vec<String> = prompt
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut citations: Vec<Citation> = snapshot
        .notes
        .iter()
        .filter(|note| selected_ids.contains(note.knowledge_base_id.as_str()))
        .filter_map(|note| {
            let searchable_text = format!(
                "{} {} {} {}",
                note.title,
                note.path,
                note.tags.join(" "),
                note.content
            )
            .to_lowercase();
            let term_score = prompt_terms
                .iter()
                .filter(|term| searchable_text.contains(term.as_str()))
                .count() as f64;
            let fallback_score = ["写入", "隐私", "检索", "agent", "本地"]
                .iter()
                .filter(|term| searchable_text.contains(*term))
                .count() as f64;
            let score = term_score * 2.0 + fallback_score;

            if score <= 0.0 {
                return None;
            }

            let knowledge_base = snapshot
                .knowledge_bases
                .iter()
                .find(|item| item.id == note.knowledge_base_id)?;

            Some(Citation {
                knowledge_base_id: note.knowledge_base_id.clone(),
                knowledge_base_name: knowledge_base.name.clone(),
                note_id: note.id.clone(),
                title: note.title.clone(),
                path: note.path.clone(),
                snippet: extract_snippet(&note.content, prompt),
                score,
            })
        })
        .collect();

    citations.sort_by(|left, right| right.score.total_cmp(&left.score));
    citations.truncate(4);
    citations
}

/** 从 Markdown 内容中提取引用片段。 */
fn extract_snippet(content: &str, prompt: &str) -> String {
    let prompt_terms: Vec<&str> = prompt
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect();

    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find(|line| prompt_terms.iter().any(|term| line.contains(term)))
        .or_else(|| {
            content
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with('#'))
        })
        .unwrap_or("命中该笔记，但暂无可展示片段。")
        .to_owned()
}

/** 创建工具调用记录。 */
fn tool_call(name: &str, summary: &str, args: serde_json::Value) -> AgentToolCall {
    tool_call_with_status(name, summary, args, "completed")
}

/** 创建指定状态的工具调用记录，用于展示失败恢复等非完成态轨迹。 */
fn tool_call_with_status(
    name: &str,
    summary: &str,
    args: serde_json::Value,
    status: &str,
) -> AgentToolCall {
    AgentToolCall {
        id: create_id("tool"),
        name: name.to_owned(),
        status: status.to_owned(),
        summary: summary.to_owned(),
        args,
    }
}

/** 创建改写建议，Agent 只能生成 diff，不能直接写文件。 */
fn propose_rewrite(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    active_note: Option<crate::domain::Note>,
    tool_calls: &mut Vec<AgentToolCall>,
) -> String {
    let Some(note) = active_note else {
        return "当前没有可改写的笔记。".to_owned();
    };
    let original = first_body_paragraph(&note.content);

    if original.is_empty() {
        return "我没有找到适合改写的正文段落。".to_owned();
    }

    let next = format!(
        "这款产品面向长期处理资料、灵感和项目文档的个人知识工作者。它以本地 Markdown 目录作为可信数据源，在保留用户文件所有权的前提下，让 Agent 负责检索、总结、改写和生成笔记；任何写入都会先展示变更预览，并在用户确认后才落盘。\n\n原段落要点：{}",
        original
    );
    snapshot.sessions[session_index].pending_change = Some(ProposedChange {
        id: create_id("change"),
        knowledge_base_id: note.knowledge_base_id.clone(),
        note_id: Some(note.id.clone()),
        r#type: "rewrite".to_owned(),
        title: format!("改写《{}》的核心段落", note.title),
        target_path: note.path.clone(),
        original,
        next,
        original_hash: note.content_hash.clone(),
        status: "pending".to_owned(),
    });
    tool_calls.push(tool_call(
        "propose_note_change",
        &format!("已为《{}》生成待确认改写 diff", note.title),
        json!({ "noteId": note.id, "targetPath": note.path }),
    ));

    "我已经生成一份改写建议。它现在只是待确认 diff，确认前不会修改本地 Markdown 文件。".to_owned()
}

/** 创建新笔记草稿建议，新文件也必须走 diff 确认。 */
fn propose_create(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    tool_calls: &mut Vec<AgentToolCall>,
) -> String {
    let target_path = "00-Inbox/上线检查清单.md".to_owned();
    snapshot.sessions[session_index].pending_change = Some(ProposedChange {
        id: create_id("change"),
        knowledge_base_id: request.active_knowledge_base_id.clone(),
        note_id: None,
        r#type: "create".to_owned(),
        title: "创建《上线检查清单》草稿".to_owned(),
        target_path: target_path.clone(),
        original: String::new(),
        next: "# 上线检查清单\n\n## 产品体验\n- Agent 回答包含工具轨迹和引用来源。\n\n## 写入安全\n- 所有 Agent 写入都先展示 diff。\n".to_owned(),
        original_hash: hash_content(""),
        status: "pending".to_owned(),
    });
    tool_calls.push(tool_call(
        "create_note_draft",
        &format!("已生成 {} 的待确认新建 diff", target_path),
        json!({ "targetPath": target_path }),
    ));

    "我已经生成新笔记草稿，但它还没有写入本地目录。确认 diff 后才会创建 Markdown 文件。".to_owned()
}

/** 提取首个可改写正文段落。 */
fn first_body_paragraph(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| line.len() > 18 && !line.starts_with('#') && !line.starts_with('-'))
        .unwrap_or("")
        .to_owned()
}
