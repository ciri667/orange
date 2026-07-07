use crate::agent_tools::{AgentToolContext, ToolOutcome, ToolRegistry};
use crate::domain::{
    AgentMessage, AgentToolCall, AgentTurnRequest, AgentTurnResult, Citation, WorkspaceSnapshot,
};
use crate::storage::create_id;
use serde_json::json;
use std::collections::HashSet;
use tauri::AppHandle;

/** 执行本地兜底 Agent turn；模型不可用时仍通过统一 ToolRegistry 访问受控工具。 */
pub fn run_agent_turn(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
) -> AgentTurnResult {
    let session_index = resolve_session_index(&snapshot, &request);
    let mut tool_calls = Vec::new();
    let mut citations = Vec::new();
    let registry = ToolRegistry::default();

    apply_first_prompt_title(&mut snapshot.sessions[session_index], &request.prompt);
    ensure_user_message_for_turn(&mut snapshot.sessions[session_index], &request);

    log::info!(
        target: "local_agent",
        "本地保守兜底开始：session={} action={} prompt_chars={}",
        request.session_id,
        request.action,
        request.prompt.chars().count()
    );

    // 本地兜底没有模型推理能力，只响应明确的非自然语言工具入口，不替模型推断自然语言意图。
    if should_use_explicit_context_action(&request) {
        let search_outcome = execute_local_tool(
            &registry,
            app,
            &mut snapshot,
            session_index,
            &request,
            "search_notes",
            json!({ "query": request.prompt }),
        );
        let first_note_id = search_outcome
            .citations
            .first()
            .map(|citation| citation.note_id.clone());

        citations.extend(search_outcome.citations.clone());
        tool_calls.push(search_outcome.call);

        if let Some(note_id) = first_note_id {
            let read_outcome = execute_local_tool(
                &registry,
                app,
                &mut snapshot,
                session_index,
                &request,
                "read_note",
                json!({ "noteId": note_id }),
            );

            citations.extend(read_outcome.citations);
            tool_calls.push(read_outcome.call);
        }
    }

    if request.action == "organize" {
        let organize_outcome = execute_local_tool(
            &registry,
            app,
            &mut snapshot,
            session_index,
            &request,
            "suggest_organization",
            json!({
                "noteId": request.active_note_id,
                "suggestion": "建议先补充稳定标签、标题层级、来源说明和相关链接；该本地兜底不会移动或改写文件。"
            }),
        );

        tool_calls.push(organize_outcome.call);
    }

    let content = build_local_response(&request, &citations, &tool_calls);
    log::debug!(
        target: "local_agent",
        "本地保守兜底完成：session={} action={} tool_count={} citation_count={}",
        request.session_id,
        request.action,
        tool_calls.len(),
        citations.len()
    );

    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content,
            action: Some(request.action),
            citations: Some(deduplicate_citations(citations)),
            tool_calls: Some(tool_calls),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

    AgentTurnResult { snapshot }
}

/** 在本地兜底中执行单个工具，并保证所有调用都经过同一 registry dispatch。 */
fn execute_local_tool(
    registry: &ToolRegistry,
    app: &AppHandle,
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    name: &str,
    args: serde_json::Value,
) -> ToolOutcome {
    let mut context = AgentToolContext {
        app: Some(app),
        snapshot,
        session_index,
        request,
    };

    registry.execute_named(&mut context, name, args)
}

/** 本地兜底只响应明确工具入口；自然语言意图必须交给真实模型判断。 */
fn should_use_explicit_context_action(request: &AgentTurnRequest) -> bool {
    request.action == "find"
}

/** 构造本地兜底回复，明确说明没有模型推理也没有隐式写入。 */
fn build_local_response(
    request: &AgentTurnRequest,
    citations: &[Citation],
    tool_calls: &[AgentToolCall],
) -> String {
    let has_failed_search = tool_calls
        .iter()
        .any(|tool_call| tool_call.name == "search_notes" && tool_call.status == "failed");

    if matches!(request.action.as_str(), "rewrite" | "create") {
        return "当前运行在本地规则兜底模式，我不会根据固定 action 自动生成写入 diff。需要写入时请启用模型，让 Agent 显式调用 propose_note_change 或 create_note_draft 工具；确认前不会修改 Markdown 文件。"
            .to_owned();
    }

    if citations.is_empty() {
        if has_failed_search {
            return "本地检索工具没有完成，因此这轮不会编造知识库依据，也不会执行任何写入。"
                .to_owned();
        }

        if tool_calls.is_empty() {
            return "当前运行在本地保守兜底模式；自然语言意图需要启用模型后由 Agent 自主判断。确认前不会修改 Markdown 文件。"
            .to_owned();
        }

        return "我没有在当前会话允许的知识库范围内找到足够相关的内容。本地兜底不会越权读取其他目录，也不会隐式写入文件。"
            .to_owned();
    }

    let titles = citations
        .iter()
        .take(3)
        .map(|citation| format!("《{}》", citation.title))
        .collect::<Vec<_>>()
        .join("、");

    format!(
        "我通过本地工具在当前会话允许范围内找到了 {} 条相关内容，主要包括 {}。本地兜底只提供可追溯参考；写入仍必须由专门工具生成待确认 diff。",
        citations.len(),
        titles
    )
}

/** 空白新会话的标题直接使用用户第一条输入，避免按知识库或文档名组装默认标题。 */
fn apply_first_prompt_title(session: &mut crate::domain::AgentSession, prompt: &str) {
    let has_user_message = session
        .messages
        .iter()
        .any(|message| message.role == "user");

    if !has_user_message && session.title.trim() == "新会话" {
        let next_title = prompt.trim();

        if !next_title.is_empty() {
            session.title = next_title.to_owned();
        }
    }
}

/** 确保本轮用户消息只出现一次；前端即时渲染的消息会通过 client_message_id 复用。 */
fn ensure_user_message_for_turn(
    session: &mut crate::domain::AgentSession,
    request: &AgentTurnRequest,
) {
    let user_message_id = request
        .client_message_id
        .clone()
        .unwrap_or_else(|| create_id("user"));

    if session
        .messages
        .iter()
        .any(|message| message.id == user_message_id && message.role == "user")
    {
        return;
    }

    session.messages.push(AgentMessage {
        id: user_message_id,
        role: "user".to_owned(),
        content: request.prompt.clone(),
        action: Some(request.action.clone()),
        citations: None,
        tool_calls: None,
    });
}

/** 根据 sessionId 查找会话；找不到时保持旧行为回退到首个会话。 */
fn resolve_session_index(snapshot: &WorkspaceSnapshot, request: &AgentTurnRequest) -> usize {
    snapshot
        .sessions
        .iter()
        .position(|session| session.id == request.session_id)
        .or_else(|| {
            snapshot
                .sessions
                .iter()
                .position(|session| session.id == snapshot.active_session_id)
        })
        .unwrap_or(0)
}

/** 去重引用，避免 search 和 read 返回同一笔记时重复展示。 */
fn deduplicate_citations(citations: Vec<Citation>) -> Vec<Citation> {
    let mut seen_note_ids = HashSet::new();
    let mut next_citations = Vec::new();

    for citation in citations {
        // search_notes 与 read_note 可能命中同一笔记，前端引用列表只需要展示一次。
        if seen_note_ids.insert(citation.note_id.clone()) {
            next_citations.push(citation);
        }
    }

    next_citations
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 本地兜底的写入类请求不能根据 action 自动创建 pending diff。 */
    #[test]
    fn local_fallback_does_not_auto_create_pending_change_for_write_action() {
        let request = AgentTurnRequest {
            prompt: "请改写当前笔记".to_owned(),
            action: "rewrite".to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
        };
        let response = build_local_response(&request, &[], &[]);

        assert!(response.contains("不会根据固定 action 自动生成写入 diff"));
    }

    /** 本地兜底不能通过 prompt 字面内容自动检索，避免伪装成 Agent 判断。 */
    #[test]
    fn local_fallback_does_not_keyword_route_plain_ask() {
        let request = AgentTurnRequest {
            prompt: "请总结当前知识库".to_owned(),
            action: "ask".to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
        };
        let response = build_local_response(&request, &[], &[]);

        assert!(!should_use_explicit_context_action(&request));
        assert!(response.contains("自然语言意图需要启用模型后由 Agent 自主判断"));
    }

    /** 前端已乐观落库的用户消息不能在本地兜底里重复追加。 */
    #[test]
    fn local_fallback_reuses_client_user_message() {
        let mut session = crate::domain::AgentSession {
            id: "session-a".to_owned(),
            title: "测试会话".to_owned(),
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: vec![AgentMessage {
                id: "user-client".to_owned(),
                role: "user".to_owned(),
                content: "已发送消息".to_owned(),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: None,
            }],
            pending_change: None,
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
        };
        let request = AgentTurnRequest {
            prompt: "已发送消息".to_owned(),
            action: "ask".to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: String::new(),
            client_message_id: Some("user-client".to_owned()),
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
        };

        ensure_user_message_for_turn(&mut session, &request);

        assert_eq!(
            session
                .messages
                .iter()
                .filter(|message| message.role == "user")
                .count(),
            1
        );
    }
}
