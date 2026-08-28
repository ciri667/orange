use crate::domain::{
    AgentSecurityLevel, AgentSecuritySettings, AgentSession, Note, ProposedChange,
    WorkspaceDocument, WorkspaceSnapshot,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage;
use crate::text_edit::{replace_unique, UniqueReplacementError};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::AppHandle;

/** 本地完全级别会话才允许自动落盘；IM 入口永远需要人工确认。 */
pub fn allows_autonomous_auto_apply(
    session: &AgentSession,
    settings: &AgentSecuritySettings,
) -> bool {
    session.im_identity.is_none()
        && AgentSecurityLevel::parse(&session.security_level).allows_auto_apply()
        && settings.autonomous_mode_enabled
}

/** 自主模式可对 Agent 直接产出的单文件 pendingChange 自动应用。 */
pub fn can_auto_apply_pending_change(
    session: &AgentSession,
    settings: &AgentSecuritySettings,
) -> bool {
    if !allows_autonomous_auto_apply(session, settings) {
        return false;
    }

    session
        .pending_change
        .as_ref()
        .is_some_and(|change| change.status == "pending")
}

/** 确认待写入 diff：校验知识库边界和内容 hash 后原子写回 Markdown/TXT。 */
pub fn apply_pending_change(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let session_id = snapshot.active_session_id.clone();
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;
    let Some(change) = snapshot.sessions[session_index].pending_change.clone() else {
        return Ok(snapshot);
    };
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == change.knowledge_base_id)
        .ok_or_else(|| "找不到变更所属知识库".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let target_path = storage::resolve_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &change.target_path,
    )?;

    let change_file_type = change.file_type.as_deref().unwrap_or("markdown");
    if !matches!(change_file_type, "markdown" | "txt") {
        return Err("待确认变更的文件类型不受支持。".to_owned());
    }

    if change.r#type == "create" {
        apply_create_change(&mut snapshot, &change, &target_path, change_file_type)?;
    } else if change_file_type == "txt" {
        apply_txt_rewrite(
            app,
            &mut snapshot,
            &change,
            &target_path,
            &session_id,
            &operation_id,
            &knowledge_base_id,
            started_at,
        )?;
    } else if let Some(note_id) = change.target_id.as_ref().or(change.note_id.as_ref()) {
        apply_markdown_rewrite(
            app,
            &mut snapshot,
            &change,
            note_id,
            &target_path,
            &session_id,
            &operation_id,
            &knowledge_base_id,
            started_at,
        )?;
    }

    let accepted_change_id = change.id.clone();
    let accepted_change_type = change.r#type.clone();
    let accepted_operation = change.operation.clone();
    let accepted_review_comment_count = change
        .review_comments
        .as_ref()
        .map(|comments| comments.len())
        .unwrap_or_default();
    let accepted_diff_hunk_count = change.diff_stats.as_ref().map(|stats| stats.hunk_count);
    let accepted_target_path = change.target_path.clone();
    snapshot.sessions[session_index].pending_change = Some(ProposedChange {
        status: "accepted".to_owned(),
        ..change
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::index_snapshot(app, &snapshot)?;

    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "apply_proposed_change",
            "completed",
            "已接受并写入 Agent diff。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .knowledge_base_id(knowledge_base_id)
        .entity("change", accepted_change_id)
        .relative_path(accepted_target_path)
        .duration(started_at.elapsed())
        .metadata(json!({
            "changeType": accepted_change_type,
            "operation": accepted_operation,
            "reviewCommentCount": accepted_review_comment_count,
            "diffHunkCount": accepted_diff_hunk_count,
        })),
    );

    Ok(snapshot)
}

/** 在落盘前执行 hash 冲突检测和唯一片段替换，确保一次确认只改一处。 */
pub fn apply_rewrite_change(
    current_content: &str,
    current_hash: &str,
    snapshot_hash: &str,
    change: &ProposedChange,
) -> Result<String, String> {
    // hash 不一致说明文件可能被外部修改，必须阻止写入并要求用户重新生成 diff。
    if current_hash != change.original_hash && snapshot_hash != change.original_hash {
        return Err("目标文件已变化，已阻止写入。请重新生成 diff。".to_owned());
    }

    if matches!(
        change.operation.as_deref(),
        Some("append" | "multi_replace")
    ) {
        if current_content != change.original {
            let action_label = if change.operation.as_deref() == Some("multi_replace") {
                "多处编辑写入"
            } else {
                "追加写入"
            };

            return Err(format!(
                "目标文件已变化，已阻止{action_label}。请重新生成 diff。"
            ));
        }

        return Ok(change.next.clone());
    }

    replace_unique(current_content, &change.original, &change.next)
        .map_err(rewrite_apply_error_message)
}

fn apply_create_change(
    snapshot: &mut WorkspaceSnapshot,
    change: &ProposedChange,
    target_path: &Path,
    change_file_type: &str,
) -> Result<(), String> {
    // 新建草稿不能覆盖用户已有文件；如路径已存在，应重新生成不同目标路径的 diff。
    if target_path.exists() {
        return Err("目标文件已存在，已阻止覆盖。请重新生成草稿路径。".to_owned());
    }

    if change_file_type == "txt" {
        storage::atomic_write_text_document(target_path, &change.next)?;
        let document_id =
            storage::create_stable_note_id(&change.knowledge_base_id, &change.target_path);
        let title = Path::new(&change.target_path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Agent 草稿")
            .to_owned();
        snapshot.documents.insert(
            0,
            WorkspaceDocument {
                id: document_id.clone(),
                knowledge_base_id: change.knowledge_base_id.clone(),
                title,
                path: change.target_path.clone(),
                file_type: "txt".to_owned(),
                updated_at: "刚刚".to_owned(),
                content_hash: storage::hash_content(&change.next),
                content: Some(change.next.clone()),
                preview_available: false,
            },
        );
        snapshot.active_note_id.clear();
        snapshot.active_document_id = document_id;
    } else {
        storage::atomic_write_markdown(target_path, &change.next)?;
        snapshot.notes.insert(
            0,
            Note {
                id: storage::create_stable_note_id(&change.knowledge_base_id, &change.target_path),
                knowledge_base_id: change.knowledge_base_id.clone(),
                title: change.title.replace("创建《", "").replace("》草稿", ""),
                path: change.target_path.clone(),
                content: change.next.clone(),
                tags: vec!["Agent".to_owned(), "草稿".to_owned()],
                updated_at: "刚刚".to_owned(),
                backlinks: Vec::new(),
                content_hash: storage::hash_content(&change.next),
            },
        );
    }

    Ok(())
}

fn apply_txt_rewrite(
    app: &AppHandle,
    snapshot: &mut WorkspaceSnapshot,
    change: &ProposedChange,
    target_path: &Path,
    session_id: &str,
    operation_id: &str,
    knowledge_base_id: &str,
    started_at: Instant,
) -> Result<(), String> {
    let document_id = change
        .target_id
        .as_ref()
        .ok_or_else(|| "待写入 TXT 缺少目标 ID。".to_owned())?;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == *document_id && document.file_type == "txt")
        .ok_or_else(|| "找不到待写入 TXT 文件。".to_owned())?;
    let fallback_content = snapshot.documents[document_index]
        .content
        .clone()
        .unwrap_or_default();
    let current_content = fs::read_to_string(target_path).unwrap_or(fallback_content);
    let current_hash = storage::hash_content(&current_content);
    let next_content = apply_rewrite_change(
        &current_content,
        &current_hash,
        &snapshot.documents[document_index].content_hash,
        change,
    )?;
    capture_history_before_write(
        app,
        storage::DocumentHistoryCapture {
            target_kind: "document".to_owned(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            target_id: document_id.clone(),
            relative_path: snapshot.documents[document_index].path.clone(),
            title: snapshot.documents[document_index].title.clone(),
            file_type: "txt".to_owned(),
            content: current_content,
            source: "agent-change".to_owned(),
            session_id: Some(session_id.to_owned()),
            change_id: Some(change.id.clone()),
            operation_id: Some(operation_id.to_owned()),
        },
        started_at,
    )?;
    storage::atomic_write_text_document(target_path, &next_content)?;
    snapshot.documents[document_index].content = Some(next_content.clone());
    snapshot.documents[document_index].content_hash = storage::hash_content(&next_content);
    snapshot.documents[document_index].updated_at = "刚刚".to_owned();
    snapshot.active_note_id.clear();
    snapshot.active_document_id = document_id.clone();
    Ok(())
}

fn apply_markdown_rewrite(
    app: &AppHandle,
    snapshot: &mut WorkspaceSnapshot,
    change: &ProposedChange,
    note_id: &str,
    target_path: &Path,
    session_id: &str,
    operation_id: &str,
    knowledge_base_id: &str,
    started_at: Instant,
) -> Result<(), String> {
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == note_id)
        .ok_or_else(|| "找不到待写入笔记".to_owned())?;
    let fallback_content = snapshot.notes[note_index].content.clone();
    let current_content = fs::read_to_string(target_path).unwrap_or(fallback_content);
    let current_hash = storage::hash_content(&current_content);
    let next_content = match apply_rewrite_change(
        &current_content,
        &current_hash,
        &snapshot.notes[note_index].content_hash,
        change,
    ) {
        Ok(next_content) => next_content,
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Agent,
                    "apply_proposed_change",
                    "blocked",
                    error.clone(),
                )
                .operation_id(operation_id.to_owned())
                .session_id(session_id.to_owned())
                .knowledge_base_id(knowledge_base_id.to_owned())
                .entity("change", change.id.clone())
                .relative_path(change.target_path.clone())
                .duration(started_at.elapsed()),
            );
            return Err(error);
        }
    };

    capture_history_before_write(
        app,
        storage::DocumentHistoryCapture {
            target_kind: "note".to_owned(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            target_id: note_id.to_owned(),
            relative_path: snapshot.notes[note_index].path.clone(),
            title: snapshot.notes[note_index].title.clone(),
            file_type: "markdown".to_owned(),
            content: current_content,
            source: "agent-change".to_owned(),
            session_id: Some(session_id.to_owned()),
            change_id: Some(change.id.clone()),
            operation_id: Some(operation_id.to_owned()),
        },
        started_at,
    )?;
    storage::atomic_write_markdown(target_path, &next_content)?;
    snapshot.notes[note_index].content = next_content.clone();
    snapshot.notes[note_index].content_hash = storage::hash_content(&next_content);
    snapshot.notes[note_index].updated_at = "刚刚".to_owned();
    Ok(())
}

/** 覆盖写入前捕获当前磁盘版本；失败会阻止后续写入，避免没有回档点。 */
fn capture_history_before_write(
    app: &AppHandle,
    capture: storage::DocumentHistoryCapture,
    started_at: Instant,
) -> Result<(), String> {
    let source = capture.source.clone();
    let byte_size = capture.content.as_bytes().len();
    let knowledge_base_id = capture.knowledge_base_id.clone();
    let target_kind = capture.target_kind.clone();
    let target_id = capture.target_id.clone();
    let relative_path = capture.relative_path.clone();

    match storage::capture_document_history(app, capture) {
        Ok(capture_summary) => {
            if capture_summary.prune_summary.cleanup_failure_count > 0 {
                logging::write_app_event_best_effort(
                    app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::Agent,
                        "apply_proposed_change",
                        "partial",
                        "文档历史已捕获，但部分过期快照清理失败。",
                    )
                    .duration(started_at.elapsed())
                    .knowledge_base_id(knowledge_base_id)
                    .entity(target_kind, target_id)
                    .relative_path(relative_path)
                    .metadata(json!({
                        "source": source,
                        "byteSize": byte_size,
                        "captured": capture_summary.entry.is_some(),
                        "removedCount": capture_summary.prune_summary.removed_count,
                        "cleanupFailureCount": capture_summary.prune_summary.cleanup_failure_count,
                    })),
                );
            }
            Ok(())
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Agent,
                    "apply_proposed_change",
                    "failed",
                    "文档历史捕获失败，已阻止覆盖写入。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(knowledge_base_id)
                .entity(target_kind, target_id)
                .relative_path(relative_path)
                .metadata(json!({
                    "source": source,
                    "byteSize": byte_size,
                })),
            );
            Err(format!("无法保存当前版本历史，已阻止覆盖写入：{error}"))
        }
    }
}

/** 将单处改写定位失败转换为用户可理解的写入错误。 */
fn rewrite_apply_error_message(error: UniqueReplacementError) -> String {
    match error {
        UniqueReplacementError::EmptyOriginal => {
            "待写入 diff 缺少原文片段，已阻止写入。请重新生成 diff。".to_owned()
        }
        UniqueReplacementError::NotFound => {
            "待写入 diff 的原文片段未命中当前文件，已阻止写入。请重新生成 diff。".to_owned()
        }
        UniqueReplacementError::Ambiguous { .. } => {
            "待写入 diff 的原文片段在当前文件中出现多次，已阻止写入。请重新生成更精确的 diff。"
                .to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AgentSecuritySettings, ImSessionIdentity, ProposedChange};

    fn session_with(level: &str, im: bool, pending: bool) -> AgentSession {
        AgentSession {
            id: "s".to_owned(),
            title: "t".to_owned(),
            im_identity: im.then(|| ImSessionIdentity {
                provider_id: "feishu".to_owned(),
                conversation_kind: "direct".to_owned(),
                channel_hash: "x".to_owned(),
                initial_message_preview: "m".to_owned(),
                last_message_preview: "m".to_owned(),
            }),
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: Vec::new(),
            pending_change: pending.then(|| ProposedChange {
                id: "change".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                note_id: Some("note-a".to_owned()),
                target_id: Some("note-a".to_owned()),
                target_kind: Some("note".to_owned()),
                file_type: Some("markdown".to_owned()),
                r#type: "rewrite".to_owned(),
                operation: Some("replace".to_owned()),
                title: "改写".to_owned(),
                target_path: "note.md".to_owned(),
                original: "旧".to_owned(),
                next: "新".to_owned(),
                original_hash: "hash".to_owned(),
                status: "pending".to_owned(),
                review_comments: None,
                review_state: None,
                diff_stats: None,
            }),
            pending_change_set: None,
            pending_execution: None,
            security_level: level.to_owned(),
            context_summary: None,
            created_at: "t".to_owned(),
            updated_at: "t".to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
            context_usage: None,
        }
    }

    /** 单文件 pendingChange 只在本地完全级别且总开关打开时自动落盘。 */
    #[test]
    fn can_auto_apply_pending_change_gate() {
        let mut settings = AgentSecuritySettings::default();
        settings.autonomous_mode_enabled = true;

        assert!(can_auto_apply_pending_change(
            &session_with("autonomous", false, true),
            &settings
        ));
        assert!(!can_auto_apply_pending_change(
            &session_with("advanced", false, true),
            &settings
        ));
        assert!(!can_auto_apply_pending_change(
            &session_with("basic", false, true),
            &settings
        ));
        assert!(!can_auto_apply_pending_change(
            &session_with("autonomous", true, true),
            &settings
        ));
        assert!(!can_auto_apply_pending_change(
            &session_with("autonomous", false, false),
            &settings
        ));

        settings.autonomous_mode_enabled = false;
        assert!(!can_auto_apply_pending_change(
            &session_with("autonomous", false, true),
            &settings
        ));
    }
}
