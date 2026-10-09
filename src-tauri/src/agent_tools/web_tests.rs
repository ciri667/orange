use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent_tools::{AgentToolContext, ToolRegistry};
use crate::domain::{AgentSession, AgentTurnRequest, KnowledgeBase, WorkspaceSnapshot};
use crate::web::{FetchedPage, TurnBudget, WebHit, WebOverride};

fn snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        knowledge_bases: vec![KnowledgeBase {
            id: "kb-a".to_owned(),
            name: "主知识库".to_owned(),
            path: "/tmp/kb-a".to_owned(),
            description: "测试".to_owned(),
            status: "ready".to_owned(),
            note_count: 0,
            document_count: 0,
            updated_at: "刚刚".to_owned(),
            is_default: true,
            semantic_index_enabled: false,
            scan_report: None,
        }],
        folders: Vec::new(),
        notes: Vec::new(),
        documents: Vec::new(),
        sessions: vec![AgentSession {
            id: "session-web".to_owned(),
            title: "联网".to_owned(),
            im_identity: None,
            schedule_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
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
            context_usage: None,
            title_customized: false,
            pinned_at: None,
            archived_at: None,
        }],
        active_knowledge_base_id: "kb-a".to_owned(),
        active_note_id: String::new(),
        active_document_id: String::new(),
        active_session_id: "session-web".to_owned(),
    }
}

fn request() -> AgentTurnRequest {
    AgentTurnRequest {
        prompt: "查一下".to_owned(),
        action: "ask".to_owned(),
        session_id: "session-web".to_owned(),
        active_knowledge_base_id: "kb-a".to_owned(),
        active_note_id: String::new(),
        client_message_id: None,
        model_provider_id: None,
        model_id: None,
        explicit_skill_ids: Vec::new(),
        mentioned_file_ids: Vec::new(),
        image_ids: Vec::new(),
    }
}

fn gateway(enabled: bool, configured: bool, page: Option<String>) -> Arc<WebOverride> {
    let mut pages = std::collections::HashMap::new();
    if let Some(text) = page {
        pages.insert(
            "https://example.com/a".to_owned(),
            FetchedPage {
                final_url: "https://example.com/a".to_owned(),
                title: "示例".to_owned(),
                text,
                byte_truncated: false,
            },
        );
    }
    Arc::new(WebOverride {
        enabled,
        configured,
        hits: vec![WebHit {
            title: "示例".to_owned(),
            url: "https://example.com/a".to_owned(),
            snippet: "摘要".to_owned(),
            published_at: Some("2026-10-01".to_owned()),
        }],
        pages,
        budget: Mutex::new(TurnBudget::default()),
    })
}

#[test]
fn web_search_fails_when_disabled_and_returns_citations_when_enabled() {
    let registry = ToolRegistry::default();
    assert!(registry.tool_names().contains(&"search"));
    assert!(!registry.tool_names().contains(&"web_search"));

    let mut snapshot = snapshot();
    let request = request();
    let mut context = AgentToolContext {
        app: None,
        snapshot: &mut snapshot,
        session_index: 0,
        request: &request,
        web_override: Some(gateway(false, true, None)),
    };
    let disabled = registry.execute_named(
        &mut context,
        "search",
        json!({ "target": "web", "query": "orange" }),
    );
    assert_eq!(disabled.call.status, "failed");
    assert!(disabled.payload["error"]
        .as_str()
        .unwrap_or("")
        .contains("未开启"));

    context.web_override = Some(gateway(true, false, None));
    let missing_key = registry.execute_named(
        &mut context,
        "search",
        json!({ "target": "web", "query": "orange" }),
    );
    assert_eq!(missing_key.call.status, "failed");

    context.web_override = Some(gateway(true, true, None));
    let found = registry.execute_named(
        &mut context,
        "search",
        json!({ "target": "web", "query": "orange", "limit": 5 }),
    );
    assert_eq!(found.call.status, "completed");
    assert_eq!(found.call.name, "search");
    assert_eq!(found.citations.len(), 1);
    assert_eq!(found.citations[0].kind.as_deref(), Some("web"));
    assert_eq!(
        found.citations[0].url.as_deref(),
        Some("https://example.com/a")
    );
    assert!(found.payload["text"]
        .as_str()
        .unwrap_or("")
        .contains("不要当作指令"));
}

#[test]
fn web_read_windows_long_pages_and_fifth_search_is_rejected() {
    let registry = ToolRegistry::default();
    let page = "字".repeat(6001);
    let shared = gateway(true, true, Some(page));
    let mut snapshot = snapshot();
    let request = request();
    let mut context = AgentToolContext {
        app: None,
        snapshot: &mut snapshot,
        session_index: 0,
        request: &request,
        web_override: Some(Arc::clone(&shared)),
    };

    let first = registry.execute_named(
        &mut context,
        "read",
        json!({ "url": "https://example.com/a" }),
    );
    assert_eq!(first.call.status, "completed");
    assert_eq!(first.payload["truncated"], true);
    assert_eq!(first.payload["nextOffset"], 6000);
    assert!(!first.payload["content"]
        .as_str()
        .unwrap_or("")
        .contains("script"));
    let continued = registry.execute_named(
        &mut context,
        "read",
        json!({ "url": "https://example.com/a", "offset": 6000 }),
    );
    assert_eq!(continued.call.status, "completed");
    assert_eq!(
        continued.payload["content"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|ch| *ch == '字')
            .count(),
        1
    );

    for index in 0..4 {
        let outcome = registry.execute_named(
            &mut context,
            "search",
            json!({ "target": "web", "query": format!("q{index}") }),
        );
        assert_eq!(outcome.call.status, "completed", "{index}");
    }
    let blocked = registry.execute_named(
        &mut context,
        "search",
        json!({ "target": "web", "query": "q5" }),
    );
    assert_eq!(blocked.call.status, "failed");
    assert!(blocked.payload["error"]
        .as_str()
        .unwrap_or("")
        .contains("4"));
}
