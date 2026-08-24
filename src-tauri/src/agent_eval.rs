//! 假 Provider 驱动的 Agent 闭集评测。改工具 description / system prompt / 循环后必跑。
#![allow(dead_code)]

use crate::agent_tools::{AgentToolContext, ToolRegistry};
use crate::domain::{
    AgentSession, AgentTurnRequest, FolderEntry, KnowledgeBase, Note, WorkspaceSnapshot,
};
use crate::provider_error::{is_length_stop, parse_finish_reason};
use crate::storage::hash_content;
use serde_json::{json, Value};

/** 一轮脚本化循环的观察结果，供五个固定剧本断言。 */
struct ScriptedLoopResult {
    tool_names: Vec<String>,
    tool_statuses: Vec<String>,
    final_content: Option<String>,
    skipped_length_batch: bool,
}

/** 用既定 chat completions JSON 驱动工具循环，不打网。 */
fn run_scripted_tool_loop(
    snapshot: &mut WorkspaceSnapshot,
    request: &AgentTurnRequest,
    registry: &ToolRegistry,
    responses: &[Value],
) -> ScriptedLoopResult {
    let mut tool_names = Vec::new();
    let mut tool_statuses = Vec::new();
    let mut skipped_length_batch = false;
    let mut final_content = None;

    for response in responses {
        let finish_reason = parse_finish_reason(response);
        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let model_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let visible = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        if is_length_stop(finish_reason.as_deref()) && !model_tool_calls.is_empty() {
            skipped_length_batch = true;
            continue;
        }

        if model_tool_calls.is_empty() {
            final_content = Some(if visible.is_empty() && is_length_stop(finish_reason.as_deref())
            {
                "模型输出因长度限制被截断，没有完整回复。请用更短的下一步继续。".to_owned()
            } else {
                visible
            });
            break;
        }

        for model_tool_call in model_tool_calls {
            let mut context = AgentToolContext {
                app: None,
                snapshot,
                session_index: 0,
                request,
            };
            let outcome = registry.execute_model_tool_call(&mut context, &model_tool_call);
            tool_names.push(outcome.call.name.clone());
            tool_statuses.push(outcome.call.status.clone());
        }
    }

    ScriptedLoopResult {
        tool_names,
        tool_statuses,
        final_content,
        skipped_length_batch,
    }
}

/** 构造带 function.arguments 的假模型 tool_call。 */
fn scripted_tool_call(id: &str, name: &str, args: Value) -> Value {
    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": args.to_string()
        }
    })
}

/** 构造 OpenAI-compatible 假响应。 */
fn scripted_response(finish_reason: &str, content: &str, tool_calls: Vec<Value>) -> Value {
    json!({
        "choices": [{
            "finish_reason": finish_reason,
            "message": {
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls
            }
        }]
    })
}

fn eval_request(action: &str, prompt: &str) -> AgentTurnRequest {
    AgentTurnRequest {
        prompt: prompt.to_owned(),
        action: action.to_owned(),
        session_id: "session-a".to_owned(),
        active_knowledge_base_id: "kb-a".to_owned(),
        active_note_id: "note-a".to_owned(),
        client_message_id: None,
        model_provider_id: None,
        model_id: None,
        explicit_skill_ids: Vec::new(),
        mentioned_file_ids: Vec::new(),
    }
}

fn eval_snapshot(note_content: String) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        knowledge_bases: vec![
            KnowledgeBase {
                id: "kb-a".to_owned(),
                name: "主知识库".to_owned(),
                path: "/tmp/kb-a".to_owned(),
                description: "评测知识库".to_owned(),
                status: "ready".to_owned(),
                note_count: 1,
                document_count: 0,
                updated_at: "刚刚".to_owned(),
                is_default: true,
                semantic_index_enabled: false,
                scan_report: None,
            },
            KnowledgeBase {
                id: "kb-b".to_owned(),
                name: "未授权知识库".to_owned(),
                path: "/tmp/kb-b".to_owned(),
                description: "库外对照".to_owned(),
                status: "ready".to_owned(),
                note_count: 1,
                document_count: 0,
                updated_at: "刚刚".to_owned(),
                is_default: false,
                semantic_index_enabled: false,
                scan_report: None,
            },
        ],
        folders: vec![FolderEntry {
            id: "folder-a".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            name: "Notes".to_owned(),
            path: "Notes".to_owned(),
            updated_at: "刚刚".to_owned(),
        }],
        notes: vec![
            Note {
                id: "note-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                title: "授权笔记".to_owned(),
                path: "Notes/授权笔记.md".to_owned(),
                content_hash: hash_content(&note_content),
                content: note_content,
                tags: vec!["评测".to_owned()],
                updated_at: "刚刚".to_owned(),
                backlinks: Vec::new(),
            },
            Note {
                id: "note-b".to_owned(),
                knowledge_base_id: "kb-b".to_owned(),
                title: "未授权笔记".to_owned(),
                path: "Private/未授权笔记.md".to_owned(),
                content_hash: hash_content("outside-scope"),
                content: "outside-scope".to_owned(),
                tags: Vec::new(),
                updated_at: "刚刚".to_owned(),
                backlinks: Vec::new(),
            },
        ],
        documents: Vec::new(),
        sessions: vec![AgentSession {
            id: "session-a".to_owned(),
            title: "评测会话".to_owned(),
            im_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: Some("note-a".to_owned()),
            pinned_note_ids: vec!["note-a".to_owned()],
            messages: Vec::new(),
            pending_change: None,
            pending_change_set: None,
            pending_execution: None,
            security_level: "basic".to_owned(),
            context_summary: None,
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
        }],
        active_knowledge_base_id: "kb-a".to_owned(),
        active_note_id: "note-a".to_owned(),
        active_document_id: String::new(),
        active_session_id: "session-a".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_tools::MAX_READ_NOTE_CHARS;
    use crate::domain::AgentSecuritySettings;

    /** 检索引用：必须 read（无 AppHandle 时 search 不可用），回复含引用，无写入。 */
    #[test]
    fn harness_ask_uses_read_and_does_not_write() {
        let mut snapshot = eval_snapshot("本地优先把 Markdown 当作主数据源。".to_owned());
        let original_hash = snapshot.notes[0].content_hash.clone();
        let request = eval_request("ask", "本地优先是什么");
        let registry = ToolRegistry::default();
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call(
                        "call-read",
                        "read",
                        json!({ "fileId": "note-a" }),
                    )],
                ),
                scripted_response(
                    "stop",
                    "根据《授权笔记》，本地优先把 Markdown 当作主数据源。",
                    vec![],
                ),
            ],
        );

        assert_eq!(result.tool_names, vec!["read".to_owned()]);
        assert_eq!(result.tool_statuses, vec!["completed".to_owned()]);
        assert!(result
            .final_content
            .unwrap_or_default()
            .contains("授权笔记"));
        assert!(snapshot.sessions[0].pending_change.is_none());
        assert_eq!(snapshot.notes[0].content_hash, original_hash);
    }

    /** 局部替换只产生 pending，确认前文件 hash 不变。 */
    #[test]
    fn harness_edit_creates_pending_without_touching_file() {
        let original = "这是一段可以被改写的正文内容。".to_owned();
        let mut snapshot = eval_snapshot(original.clone());
        let original_hash = snapshot.notes[0].content_hash.clone();
        let request = eval_request("rewrite", "改写这段");
        let registry = ToolRegistry::default();
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call(
                        "call-edit",
                        "edit",
                        json!({
                            "fileId": "note-a",
                            "operation": "replace",
                            "original": original,
                            "next": "这是改写后的正文。"
                        }),
                    )],
                ),
                scripted_response("stop", "已生成待确认 diff。", vec![]),
            ],
        );

        assert_eq!(result.tool_names, vec!["edit".to_owned()]);
        assert_eq!(result.tool_statuses, vec!["completed".to_owned()]);
        let pending = snapshot.sessions[0].pending_change.as_ref().unwrap();
        assert_eq!(pending.status, "pending");
        assert_eq!(pending.next, "这是改写后的正文。");
        assert_eq!(snapshot.notes[0].content, original);
        assert_eq!(snapshot.notes[0].content_hash, original_hash);
    }

    /** 超长文件第一次 read 截断，第二次带 offset 续读。 */
    #[test]
    fn harness_truncated_read_continues_with_offset() {
        let content = format!("HEAD{}", "TAIL".repeat(MAX_READ_NOTE_CHARS));
        let mut snapshot = eval_snapshot(content);
        let request = eval_request("ask", "读完全文");
        let registry = ToolRegistry::default();
        let first = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[scripted_response(
                "stop",
                "",
                vec![scripted_tool_call(
                    "call-read-1",
                    "read",
                    json!({ "fileId": "note-a" }),
                )],
            )],
        );
        assert_eq!(first.tool_names, vec!["read".to_owned()]);

        let mut context = AgentToolContext {
            app: None,
            snapshot: &mut snapshot,
            session_index: 0,
            request: &request,
        };
        let first_outcome = registry.execute_named(&mut context, "read", json!({ "fileId": "note-a" }));
        assert_eq!(first_outcome.payload["truncated"], true);
        let next_offset = first_outcome.payload["nextOffset"].as_u64().unwrap();
        let first_text = first_outcome.payload["note"]["content"]
            .as_str()
            .unwrap_or_default();
        assert!(first_text.starts_with("HEAD"));

        let second_outcome = registry.execute_named(
            &mut context,
            "read",
            json!({ "fileId": "note-a", "offset": next_offset }),
        );
        let second_text = second_outcome.payload["note"]["content"]
            .as_str()
            .unwrap_or_default();
        assert!(!second_text.contains("HEAD"));
        assert!(second_text.contains("TAIL"));
    }

    /** 目标不在会话知识库时工具失败，无 pending。 */
    #[test]
    fn harness_scope_reject_does_not_create_pending() {
        let mut snapshot = eval_snapshot("授权正文".to_owned());
        let request = eval_request("ask", "读外面的笔记");
        let registry = ToolRegistry::default();
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call(
                        "call-read",
                        "read",
                        json!({ "fileId": "note-b" }),
                    )],
                ),
                scripted_response("stop", "无法读取未授权笔记。", vec![]),
            ],
        );

        assert_eq!(result.tool_names, vec!["read".to_owned()]);
        assert_eq!(result.tool_statuses, vec!["failed".to_owned()]);
        assert!(snapshot.sessions[0].pending_change.is_none());
    }

    /** 基础级别 schema 无 run；进阶 run 在无沙箱时失败且不改知识库。 */
    #[test]
    fn harness_basic_schema_has_no_run_and_advanced_run_does_not_write_kb() {
        let mut snapshot = eval_snapshot("授权正文".to_owned());
        let original_hash = snapshot.notes[0].content_hash.clone();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        let basic = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        assert!(!basic.tool_names().contains(&"run"));

        snapshot.sessions[0].security_level = "advanced".to_owned();
        let advanced = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        assert!(advanced.tool_names().contains(&"run"));

        let request = eval_request("ask", "跑 skill");
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &advanced,
            &[
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call(
                        "call-run",
                        "run",
                        json!({ "skillId": "skill-note-research" }),
                    )],
                ),
                scripted_response("stop", "需要确认后才能执行 Skill。", vec![]),
            ],
        );

        assert_eq!(result.tool_names, vec!["run".to_owned()]);
        assert_eq!(result.tool_statuses, vec!["failed".to_owned()]);
        assert!(snapshot.sessions[0].pending_execution.is_none());
        assert_eq!(snapshot.notes[0].content_hash, original_hash);
    }

    /** length 截断的残缺 edit 不得执行，文件系统和 pending 不变。 */
    #[test]
    fn harness_length_stop_rejects_truncated_edit_batch() {
        let original = "这是一段可以被改写的正文内容。".to_owned();
        let mut snapshot = eval_snapshot(original.clone());
        let request = eval_request("rewrite", "改写");
        let registry = ToolRegistry::default();
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[scripted_response(
                "length",
                "",
                vec![scripted_tool_call(
                    "call-edit",
                    "edit",
                    json!({
                        "fileId": "note-a",
                        "operation": "replace",
                        "original": original,
                        "next": "截断的"
                    }),
                )],
            )],
        );

        assert!(result.skipped_length_batch);
        assert!(result.tool_names.is_empty());
        assert!(snapshot.sessions[0].pending_change.is_none());
        assert_eq!(snapshot.notes[0].content, "这是一段可以被改写的正文内容。");
    }

    /** 四次 read 不被 3 轮硬帽打断（脚本循环本身无硬帽，锁住闭集续跑契约）。 */
    #[test]
    fn harness_multiple_reads_are_not_capped_at_three() {
        let mut snapshot = eval_snapshot("分段正文用于多次读取。".to_owned());
        let request = eval_request("ask", "多轮读取");
        let registry = ToolRegistry::default();
        let result = run_scripted_tool_loop(
            &mut snapshot,
            &request,
            &registry,
            &[
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call("r1", "read", json!({ "fileId": "note-a" }))],
                ),
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call("r2", "read", json!({ "fileId": "note-a" }))],
                ),
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call("r3", "read", json!({ "fileId": "note-a" }))],
                ),
                scripted_response(
                    "stop",
                    "",
                    vec![scripted_tool_call("r4", "read", json!({ "fileId": "note-a" }))],
                ),
                scripted_response("stop", "已读完四次。", vec![]),
            ],
        );

        assert_eq!(result.tool_names.len(), 4);
        assert!(result.tool_names.iter().all(|name| name == "read"));
        assert_eq!(result.final_content.as_deref(), Some("已读完四次。"));
    }
}
