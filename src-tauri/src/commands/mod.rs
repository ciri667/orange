pub mod agent;
mod common;
pub mod documents;
pub mod history;
pub mod im;
pub mod knowledge;
pub mod logs;
pub mod notes;
pub mod sessions;
pub mod settings;
pub mod skills;
pub mod workspace;

pub(crate) use agent::{
    handle_im_builtin_command, handle_im_pending_change_command, run_agent_turn_from_im,
    short_change_code,
};

#[cfg(test)]
mod tests {
    use super::agent::{build_im_change_details, short_change_code};
    use super::knowledge::normalize_sessions_after_rescan;
    use crate::domain::{AgentSession, KnowledgeBase, Note, ProposedChange, WorkspaceSnapshot};
    use crate::storage;

    /** 构造 commands 单元测试使用的最小知识库。 */
    fn test_knowledge_base(id: &str) -> KnowledgeBase {
        KnowledgeBase {
            id: id.to_owned(),
            name: format!("知识库 {id}"),
            path: format!("/tmp/{id}"),
            description: "测试知识库".to_owned(),
            status: "ready".to_owned(),
            note_count: 1,
            document_count: 0,
            updated_at: "刚刚".to_owned(),
            is_default: id == "kb-a",
            semantic_index_enabled: false,
            scan_report: None,
        }
    }

    /** 构造 commands 单元测试使用的最小笔记。 */
    fn test_note(id: &str, knowledge_base_id: &str) -> Note {
        Note {
            id: id.to_owned(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            title: format!("笔记 {id}"),
            path: format!("{id}.md"),
            content: format!("# 笔记 {id}"),
            tags: Vec::new(),
            updated_at: "刚刚".to_owned(),
            backlinks: Vec::new(),
            content_hash: storage::hash_content(&format!("# 笔记 {id}")),
        }
    }

    /** 构造 commands 单元测试使用的多知识库会话。 */
    fn test_session() -> AgentSession {
        AgentSession {
            id: "session-a".to_owned(),
            title: "多知识库会话".to_owned(),
            im_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned(), "kb-b".to_owned()],
            active_note_id: Some("note-b".to_owned()),
            pinned_note_ids: vec![
                "note-a".to_owned(),
                "note-b".to_owned(),
                "missing-note".to_owned(),
            ],
            messages: Vec::new(),
            pending_change: Some(ProposedChange {
                id: "change-a".to_owned(),
                knowledge_base_id: "kb-b".to_owned(),
                note_id: Some("note-b".to_owned()),
                target_id: Some("note-b".to_owned()),
                target_kind: Some("note".to_owned()),
                file_type: Some("markdown".to_owned()),
                r#type: "rewrite".to_owned(),
                operation: Some("replace".to_owned()),
                title: "改写 note-b".to_owned(),
                target_path: "note-b.md".to_owned(),
                original: "旧内容".to_owned(),
                next: "新内容".to_owned(),
                original_hash: storage::hash_content("旧内容"),
                status: "pending".to_owned(),
                review_comments: None,
                review_state: None,
                diff_stats: None,
            }),
            pending_change_set: None,
            pending_execution: None,
            security_level: "basic".to_owned(),
            context_summary: None,
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
        }
    }

    /** 构造可直接喂给 apply_rewrite_change 的待确认改写。 */
    fn test_rewrite_change(original: &str, next: &str, original_hash: &str) -> ProposedChange {
        ProposedChange {
            id: "change-test".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            note_id: Some("note-a".to_owned()),
            target_id: Some("note-a".to_owned()),
            target_kind: Some("note".to_owned()),
            file_type: Some("markdown".to_owned()),
            r#type: "rewrite".to_owned(),
            operation: Some("replace".to_owned()),
            title: "改写 note-a".to_owned(),
            target_path: "note-a.md".to_owned(),
            original: original.to_owned(),
            next: next.to_owned(),
            original_hash: original_hash.to_owned(),
            status: "pending".to_owned(),
            review_comments: None,
            review_state: None,
            diff_stats: None,
        }
    }

    /** IM 短编号必须稳定截断，详情只在显式请求时返回有限正文。 */
    #[test]
    fn im_change_details_uses_short_code_and_truncates_diff() {
        let mut change = test_rewrite_change(&"旧内容".repeat(500), &"新内容".repeat(500), "hash");
        change.id = "change-1234567890-long".to_owned();
        change.target_path = "notes/remote.md".to_owned();

        let details = build_im_change_details(&change);

        assert_eq!(short_change_code(&change.id), "change-12345");
        assert!(details.contains("目标：notes/remote.md"));
        assert!(details.chars().count() < 1_500);
    }

    /** 重扫单个知识库不能误删多知识库会话中仍然有效的其他知识库笔记引用。 */
    #[test]
    fn rescan_preserves_valid_references_from_other_scoped_knowledge_bases() {
        let mut snapshot = WorkspaceSnapshot {
            knowledge_bases: vec![test_knowledge_base("kb-a"), test_knowledge_base("kb-b")],
            folders: Vec::new(),
            notes: vec![test_note("note-a", "kb-a"), test_note("note-b", "kb-b")],
            documents: Vec::new(),
            sessions: vec![test_session()],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        };

        normalize_sessions_after_rescan(&mut snapshot, "kb-a");

        assert_eq!(
            snapshot.sessions[0].active_note_id.as_deref(),
            Some("note-b")
        );
        assert_eq!(
            snapshot.sessions[0].pinned_note_ids,
            vec!["note-a".to_owned(), "note-b".to_owned()]
        );
        assert_eq!(
            snapshot.sessions[0]
                .pending_change
                .as_ref()
                .and_then(|change| change.note_id.as_deref()),
            Some("note-b")
        );
    }

    /** 应用 rewrite 时必须只替换唯一命中的那一处。 */
    #[test]
    fn apply_rewrite_change_replaces_single_match_once() {
        let current_content = "开头\n旧段落\n结尾";
        let current_hash = storage::hash_content(current_content);
        let change = test_rewrite_change("旧段落", "新段落", &current_hash);

        let next_content = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &current_hash,
            &change,
        )
        .unwrap();

        assert_eq!(next_content, "开头\n新段落\n结尾");
    }

    /** 当前文件中原文片段出现多次时必须拒绝写入，避免一次确认误改多处。 */
    #[test]
    fn apply_rewrite_change_rejects_ambiguous_original() {
        let current_content = "旧段落\n中间\n旧段落";
        let current_hash = storage::hash_content(current_content);
        let change = test_rewrite_change("旧段落", "新段落", &current_hash);

        let result = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &current_hash,
            &change,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("出现多次"));
    }

    /** hash 冲突必须优先拒绝，避免基于过期 diff 写入外部已修改文件。 */
    #[test]
    fn apply_rewrite_change_rejects_hash_mismatch_before_replacement() {
        let current_content = "旧段落\n旧段落";
        let current_hash = storage::hash_content(current_content);
        let stale_hash = storage::hash_content("旧段落");
        let change = test_rewrite_change("旧段落", "新段落", &stale_hash);

        let result = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &storage::hash_content("snapshot changed"),
            &change,
        );

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "目标文件已变化，已阻止写入。请重新生成 diff。"
        );
    }

    /** append 变更确认时按整篇原文替换为整篇新内容，不执行局部片段替换。 */
    #[test]
    fn apply_rewrite_change_accepts_append_operation() {
        let current_content = "第一段\n第二段";
        let current_hash = storage::hash_content(current_content);
        let mut change =
            test_rewrite_change(current_content, "第一段\n第二段\n\n新增段落", &current_hash);

        change.operation = Some("append".to_owned());

        let next_content = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &current_hash,
            &change,
        )
        .unwrap();

        assert_eq!(next_content, "第一段\n第二段\n\n新增段落");
    }

    /** append 原文必须仍等于当前文件，避免基于过期整篇快照追加。 */
    #[test]
    fn apply_rewrite_change_rejects_stale_append_original() {
        let current_content = "第一段\n第二段\n外部新增";
        let snapshot_content = "第一段\n第二段";
        let snapshot_hash = storage::hash_content(snapshot_content);
        let current_hash = storage::hash_content(current_content);
        let mut change = test_rewrite_change(
            snapshot_content,
            "第一段\n第二段\n\n新增段落",
            &snapshot_hash,
        );

        change.operation = Some("append".to_owned());

        let result = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &snapshot_hash,
            &change,
        );

        assert_eq!(
            result.unwrap_err(),
            "目标文件已变化，已阻止追加写入。请重新生成 diff。"
        );
    }

    /** multi_replace 变更确认时按整篇快照替换，避免再次执行局部替换造成重复或漏改。 */
    #[test]
    fn apply_rewrite_change_accepts_multi_replace_operation() {
        let current_content = "标题\n重复一\n正文\n重复二\n结尾";
        let current_hash = storage::hash_content(current_content);
        let mut change = test_rewrite_change(current_content, "标题\n正文\n结尾", &current_hash);

        change.operation = Some("multi_replace".to_owned());

        let next_content = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &current_hash,
            &change,
        )
        .unwrap();

        assert_eq!(next_content, "标题\n正文\n结尾");
    }

    /** multi_replace 必须拒绝过期整篇原文，避免基于旧快照覆盖用户新改动。 */
    #[test]
    fn apply_rewrite_change_rejects_stale_multi_replace_original() {
        let snapshot_content = "标题\n重复一\n正文\n重复二\n结尾";
        let current_content = "标题\n重复一\n正文\n用户新改动\n重复二\n结尾";
        let snapshot_hash = storage::hash_content(snapshot_content);
        let current_hash = storage::hash_content(current_content);
        let mut change = test_rewrite_change(snapshot_content, "标题\n正文\n结尾", &snapshot_hash);

        change.operation = Some("multi_replace".to_owned());

        let result = crate::agent_writes::apply_rewrite_change(
            current_content,
            &current_hash,
            &snapshot_hash,
            &change,
        );

        assert_eq!(
            result.unwrap_err(),
            "目标文件已变化，已阻止多处编辑写入。请重新生成 diff。"
        );
    }
}
