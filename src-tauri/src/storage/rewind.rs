use super::format_local_datetime;
use crate::domain::{AgentSession, WorkspaceSnapshot};

/** 将会话截断到指定用户消息（含该条），替换正文并丢掉未确认写入。 */
pub fn rewind_session_to_user_message(
    session: &mut AgentSession,
    message_id: &str,
    prompt: &str,
) -> Result<(), String> {
    if session.im_identity.is_some() {
        return Err("即时通讯会话不支持编辑历史消息。".to_owned());
    }

    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("消息不能为空。".to_owned());
    }

    let message_index = session
        .messages
        .iter()
        .position(|message| message.id == message_id)
        .ok_or_else(|| "找不到要编辑的用户消息。".to_owned())?;
    if session.messages[message_index].role != "user" {
        return Err("只能编辑用户消息。".to_owned());
    }

    let old_content = session.messages[message_index].content.clone();
    let is_first_user_message = session
        .messages
        .iter()
        .find(|message| message.role == "user")
        .is_some_and(|message| message.id == message_id);

    session.messages.truncate(message_index + 1);
    session.messages[message_index].content = prompt.to_owned();
    session.pending_change = None;
    session.pending_change_set = None;
    session.pending_execution = None;
    reconcile_context_summary_after_rewind(session);

    if is_first_user_message && session.title.trim() == old_content.trim() {
        session.title = prompt.to_owned();
    }

    session.updated_at = format_local_datetime();
    Ok(())
}

/** 在工作台快照里截断指定会话；不读写 SQLite。 */
pub fn apply_rewind_to_snapshot(
    snapshot: &mut WorkspaceSnapshot,
    session_id: &str,
    message_id: &str,
    prompt: &str,
) -> Result<(), String> {
    let session = snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "找不到要编辑的会话。".to_owned())?;

    rewind_session_to_user_message(session, message_id, prompt)
}

/** 压缩切点若已不在保留前缀里，丢掉整份工作记忆，避免检查点描述已删除的轮次。 */
fn reconcile_context_summary_after_rewind(session: &mut AgentSession) {
    let Some(summary) = session.context_summary.as_mut() else {
        return;
    };
    let retained_ids = session
        .messages
        .iter()
        .map(|message| message.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let compacted_id = summary.last_compacted_message_id.as_deref().unwrap_or("");
    if compacted_id.is_empty() || !retained_ids.contains(compacted_id) {
        session.context_summary = None;
        return;
    }

    if summary
        .last_summarized_message_id
        .as_deref()
        .is_some_and(|id| !retained_ids.contains(id))
    {
        summary.last_summarized_message_id =
            session.messages.last().map(|message| message.id.clone());
    }
    summary.pending_change_summary = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AgentContextSummary, AgentMessage, AgentSession, ImSessionIdentity, ProposedChangeSet,
        SkillExecutionRequest, WorkspaceSnapshot,
    };

    fn test_message(id: &str, role: &str, content: &str) -> AgentMessage {
        AgentMessage {
            id: id.to_owned(),
            role: role.to_owned(),
            content: content.to_owned(),
            action: Some("ask".to_owned()),
            citations: None,
            tool_calls: None,
            mentioned_file_ids: Vec::new(),
            trace: Vec::new(),
            turn_duration_ms: None,
            interrupted: false,
        }
    }

    fn test_session(messages: Vec<AgentMessage>) -> AgentSession {
        AgentSession {
            id: "session-a".to_owned(),
            title: "第一句".to_owned(),
            im_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages,
            pending_change: None,
            pending_change_set: None,
            pending_execution: None,
            security_level: "basic".to_owned(),
            context_summary: None,
            created_at: "2026/01/01 10:00".to_owned(),
            updated_at: "2026/01/01 10:00".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
            context_usage: None,
        }
    }

    fn two_turn_session() -> AgentSession {
        test_session(vec![
            test_message("user-1", "user", "第一句"),
            test_message("assistant-1", "assistant", "第一答"),
            test_message("user-2", "user", "第二句"),
            test_message("assistant-2", "assistant", "第二答"),
        ])
    }

    #[test]
    fn rewind_truncates_after_edited_user_message_and_replaces_content() {
        let mut session = two_turn_session();
        session.messages[0].mentioned_file_ids = vec!["note-a".to_owned()];

        rewind_session_to_user_message(&mut session, "user-1", "  改过的第一句  ").unwrap();

        let ids = session
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["user-1"]);
        assert_eq!(session.messages[0].content, "改过的第一句");
        assert_eq!(
            session.messages[0].mentioned_file_ids,
            vec!["note-a".to_owned()]
        );
        assert_eq!(session.messages[0].action.as_deref(), Some("ask"));
        assert_eq!(session.title, "改过的第一句");
    }

    #[test]
    fn rewind_keeps_earlier_turns_when_editing_later_user_message() {
        let mut session = two_turn_session();

        rewind_session_to_user_message(&mut session, "user-2", "改过的第二句").unwrap();

        let ids = session
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["user-1", "assistant-1", "user-2"]);
        assert_eq!(session.messages[2].content, "改过的第二句");
        assert_eq!(session.title, "第一句");
    }

    #[test]
    fn rewind_does_not_rename_when_title_no_longer_matches_first_prompt() {
        let mut session = two_turn_session();
        session.title = "手动标题".to_owned();

        rewind_session_to_user_message(&mut session, "user-1", "改过的第一句").unwrap();

        assert_eq!(session.title, "手动标题");
    }

    #[test]
    fn rewind_drops_pending_writes_and_skill_approval() {
        let mut session = two_turn_session();
        session.pending_change_set = Some(ProposedChangeSet {
            id: "change-set-a".to_owned(),
            execution_id: "agent-direct".to_owned(),
            skill_id: "agent".to_owned(),
            status: "pending".to_owned(),
            summary: "待确认".to_owned(),
            operations: Vec::new(),
            warnings: Vec::new(),
            created_at: "2026/01/01 10:00".to_owned(),
        });
        session.pending_execution = Some(SkillExecutionRequest {
            id: "exec-a".to_owned(),
            skill_id: "skill-a".to_owned(),
            skill_name: "测试 Skill".to_owned(),
            package_hash: "hash".to_owned(),
            runtime: "python".to_owned(),
            command_preview: "python main.py".to_owned(),
            args: Vec::new(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            network_domains: Vec::new(),
            credential_aliases: Vec::new(),
            status: "pending".to_owned(),
            created_at: "2026/01/01 10:00".to_owned(),
        });

        rewind_session_to_user_message(&mut session, "user-1", "改过的第一句").unwrap();

        assert!(session.pending_change.is_none());
        assert!(session.pending_change_set.is_none());
        assert!(session.pending_execution.is_none());
    }

    #[test]
    fn rewind_keeps_checkpoint_when_compacted_message_still_retained() {
        let mut session = two_turn_session();
        session.context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026/01/01 10:00".to_owned(),
            current_goal: Some("整理笔记".to_owned()),
            last_compacted_message_id: Some("assistant-1".to_owned()),
            last_summarized_message_id: Some("assistant-2".to_owned()),
            pending_change_summary: Some("待写文件".to_owned()),
            ..AgentContextSummary::default()
        });

        rewind_session_to_user_message(&mut session, "user-2", "改过的第二句").unwrap();

        let summary = session.context_summary.as_ref().unwrap();
        assert_eq!(
            summary.last_compacted_message_id.as_deref(),
            Some("assistant-1")
        );
        assert_eq!(
            summary.last_summarized_message_id.as_deref(),
            Some("user-2")
        );
        assert!(summary.pending_change_summary.is_none());
        assert_eq!(summary.current_goal.as_deref(), Some("整理笔记"));
    }

    #[test]
    fn rewind_clears_summary_when_compacted_message_is_dropped() {
        let mut session = two_turn_session();
        session.context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026/01/01 10:00".to_owned(),
            current_goal: Some("整理笔记".to_owned()),
            last_compacted_message_id: Some("assistant-1".to_owned()),
            last_summarized_message_id: Some("assistant-1".to_owned()),
            ..AgentContextSummary::default()
        });

        rewind_session_to_user_message(&mut session, "user-1", "改过的第一句").unwrap();

        assert!(session.context_summary.is_none());
    }

    #[test]
    fn rewind_rejects_blank_prompt_unknown_message_assistant_and_im_session() {
        let mut session = two_turn_session();
        assert_eq!(
            rewind_session_to_user_message(&mut session, "user-1", "   ").unwrap_err(),
            "消息不能为空。"
        );
        assert_eq!(
            rewind_session_to_user_message(&mut session, "missing", "hello").unwrap_err(),
            "找不到要编辑的用户消息。"
        );
        assert_eq!(
            rewind_session_to_user_message(&mut session, "assistant-1", "hello").unwrap_err(),
            "只能编辑用户消息。"
        );

        session.im_identity = Some(ImSessionIdentity {
            provider_id: "feishu".to_owned(),
            conversation_kind: "direct".to_owned(),
            channel_hash: "abc".to_owned(),
            initial_message_preview: "hi".to_owned(),
            last_message_preview: "hi".to_owned(),
        });
        assert_eq!(
            rewind_session_to_user_message(&mut session, "user-1", "hello").unwrap_err(),
            "即时通讯会话不支持编辑历史消息。"
        );
    }

    #[test]
    fn apply_rewind_to_snapshot_updates_matching_session_only() {
        let mut snapshot = WorkspaceSnapshot {
            knowledge_bases: Vec::new(),
            folders: Vec::new(),
            notes: Vec::new(),
            documents: Vec::new(),
            sessions: vec![
                two_turn_session(),
                test_session(vec![test_message("user-x", "user", "另一会话")]),
            ],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        };
        snapshot.sessions[1].id = "session-b".to_owned();

        apply_rewind_to_snapshot(&mut snapshot, "session-a", "user-1", "改过的第一句").unwrap();

        assert_eq!(snapshot.sessions[0].messages.len(), 1);
        assert_eq!(snapshot.sessions[1].messages.len(), 1);
        assert_eq!(
            apply_rewind_to_snapshot(&mut snapshot, "missing", "user-1", "x").unwrap_err(),
            "找不到要编辑的会话。"
        );
    }

    #[test]
    fn delete_agent_session_transcript_removes_existing_row() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE agent_session_transcripts (
                  session_id TEXT PRIMARY KEY,
                  payload_json TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );",
            )
            .unwrap();
        crate::storage::persist_agent_session_transcript(
            &connection,
            "session-a",
            &[serde_json::json!({"role":"user","content":"old"})],
        )
        .unwrap();

        crate::storage::delete_agent_session_transcript_on_connection(&connection, "session-a")
            .unwrap();
        crate::storage::delete_agent_session_transcript_on_connection(&connection, "session-a")
            .unwrap();

        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_session_transcripts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
}
