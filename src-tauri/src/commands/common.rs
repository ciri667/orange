#![allow(unused_imports)]

pub(super) use crate::domain::{
    AgentSession, AgentSkill, AgentTurnPayload, AgentTurnResult, AppEventLog, ChangePayload,
    ClearDocumentHistoryPayload, CompactAgentContextPayload, CreateDocumentPayload,
    CreateFolderPayload, CreateNotePayload, CreateProjectInstructionPayload,
    DeleteAgentSkillPayload, DeleteDocumentPayload, DeleteKnowledgeBaseMemoryPayload,
    DeleteNotePayload, DeleteSessionPayload, DocumentHistoryEntry, DocumentHistoryEntryDetail,
    DocumentPreview, FeishuCredentialStatus, FeishuGatewayStatus, FolderEntry, ImGatewayStatus,
    ImIntegrationSettings, ImProviderCredentialStatus, ImProviderPayload, InstallAgentSkillPayload,
    InstallAgentSkillResult, KnowledgeBaseMemory, KnowledgeBaseSelection,
    LlmProviderModelRefreshResult, LoadAgentPromptDumpPayload, LoadAppEventLogsPayload,
    LoadDocumentHistoryEntryPayload, LoadDocumentHistoryPayload, LoadDocumentPreviewPayload,
    LoadSessionsPayload, ModelApiKeyStatus, OnlineSkillPreview, OnlineSkillSearchResult,
    PreviewOnlineSkillPayload, ProposedChange, RefreshLlmProviderModelsPayload,
    RemoveKnowledgeBasePayload, RenameDocumentPayload, RenameNotePayload, RequestAuditLog,
    RescanKnowledgeBasePayload, RestoreDocumentHistoryEntryPayload, RestoreSessionContextPayload,
    SaveAgentSkillPayload, SaveDocumentContentPayload, SaveFeishuSecretPayload,
    SaveImProviderSecretPayload, SaveImSettingsPayload, SaveKnowledgeBaseMemoryPayload,
    SaveModelApiKeyPayload, SaveNoteContentPayload, SaveNoteImageAttachmentsPayload,
    SaveSessionPayload, SaveUserSettingsPayload, ScanKnowledgeBasePayload, ScanReport,
    SearchOnlineSkillsPayload, ToggleAgentSkillPayload, UpdateSessionScopePayload, UserSettings,
    WorkspaceBootstrapState, WorkspaceEditorState, WorkspaceSnapshot, IM_PROVIDER_FEISHU,
};
pub(super) use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
pub(super) use crate::model_provider::{self, ProviderTemplate};
pub(super) use crate::runtime;
pub(super) use crate::skill_execution;
pub(super) use crate::skills;
pub(super) use crate::storage;

pub(super) use serde_json::json;
pub(super) use std::fs;
pub(super) use std::io::Read;
pub(super) use std::path::{Path, PathBuf};
pub(super) use std::process::Command;
pub(super) use std::time::{Duration, Instant};
pub(super) use tauri::{AppHandle, Manager};
pub(super) use tauri_plugin_dialog::DialogExt;

/** 读取文件系统真实修改时间；读取失败时记录脱敏日志并退回当前本地时间。 */
pub(super) fn read_file_updated_at_or_now(
    app: &AppHandle,
    event: &str,
    knowledge_base_id: &str,
    entity_kind: &str,
    entity_id: &str,
    relative_path: &str,
    path: &Path,
) -> String {
    match storage::file_modified_local_datetime(path) {
        Ok(updated_at) => updated_at,
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    event,
                    "metadata_fallback",
                    "无法读取文件系统修改时间，已退回当前本地时间。",
                )
                .knowledge_base_id(knowledge_base_id.to_owned())
                .entity(entity_kind, entity_id.to_owned())
                .relative_path(relative_path.to_owned())
                .metadata(json!({
                    "reason": "modified_time_unavailable",
                    "error": error,
                })),
            );

            storage::format_local_datetime()
        }
    }
}

/** 覆盖写入前捕获当前磁盘版本；失败会阻止后续写入，避免没有回档点。 */
pub(super) async fn capture_document_history_before_write(
    app: &AppHandle,
    capture: storage::DocumentHistoryCapture,
    log_category: AppLogCategory,
    event: &'static str,
    started_at: Instant,
) -> Result<(), String> {
    let capture_app = app.clone();
    let source = capture.source.clone();
    let byte_size = capture.content.as_bytes().len();
    let knowledge_base_id = capture.knowledge_base_id.clone();
    let target_kind = capture.target_kind.clone();
    let target_id = capture.target_id.clone();
    let relative_path = capture.relative_path.clone();

    let capture_result = run_blocking("保存文档历史记录", move || {
        storage::capture_document_history(&capture_app, capture)
    })
    .await;

    match capture_result {
        Ok(capture_summary) => {
            if capture_summary.prune_summary.cleanup_failure_count > 0 {
                logging::write_app_event_best_effort(
                    app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        log_category,
                        event,
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
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    log_category,
                    event,
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

            return Err(format!("无法保存当前版本历史，已阻止覆盖写入：{error}"));
        }
    }

    Ok(())
}

/** 删除文件成功后尽力清理其历史快照；失败只写日志，不回滚用户删除操作。 */
pub(super) async fn clear_document_history_after_delete_best_effort(
    app: &AppHandle,
    target_kind: &'static str,
    target_id: String,
    knowledge_base_id: String,
    relative_path: String,
    started_at: Instant,
) {
    let cleanup_app = app.clone();
    let target_id_for_cleanup = target_id.clone();
    let cleanup_result = run_blocking("清理已删除文件历史", move || {
        storage::clear_document_history(&cleanup_app, target_kind, &target_id_for_cleanup)
    })
    .await;

    match cleanup_result {
        Ok(summary) if summary.cleanup_failure_count > 0 => logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "document_history_cleanup",
                "failed",
                "部分历史快照清理失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id)
            .entity(target_kind, target_id)
            .relative_path(relative_path)
            .metadata(json!({
                "removedCount": summary.removed_count,
                "cleanupFailureCount": summary.cleanup_failure_count,
            })),
        ),
        Ok(_) => {}
        Err(_) => logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "document_history_cleanup",
                "failed",
                "文件删除后历史记录清理失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id)
            .entity(target_kind, target_id)
            .relative_path(relative_path)
            .metadata(json!({
                "targetKind": target_kind,
                "failureKind": "clear_failed",
            })),
        ),
    }
}

/** 将当前已连接知识库目录加入 Tauri asset 协议 scope，供 Markdown 预览加载本地图片。 */
pub(super) fn allow_asset_protocol_for_knowledge_bases(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
) -> Result<(), String> {
    for knowledge_base in &snapshot.knowledge_bases {
        if knowledge_base.status != "ready" {
            continue;
        }

        allow_asset_protocol_directory(app, Path::new(&knowledge_base.path))?;
    }

    Ok(())
}

/** 允许 asset 协议递归读取单个知识库目录；失败时返回可展示的 Tauri scope 错误。 */
pub(super) fn allow_asset_protocol_directory(app: &AppHandle, path: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_directory(path, true)
        .map_err(|error| format!("无法授权 Markdown 图片预览目录 {}：{error}", path.display()))
}

/** 在 Tauri 后台阻塞线程中运行文件系统或 SQLite 重任务，避免卡住 WebView 主线程。 */
pub(super) async fn run_blocking<T, F>(label: &str, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("{label}后台任务失败：{error}"))?
}

/** 使用系统文件管理器打开目录；失败时返回命令层错误，前端可展示路径供用户手动访问。 */
pub(super) fn open_folder_in_system(path: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");

        command.arg(path);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("explorer");

        command.arg(path);
        command
    } else {
        let mut command = Command::new("xdg-open");

        command.arg(path);
        command
    };

    // 只拉起系统文件管理器，不等待窗口生命周期，避免阻塞 Tauri 后台任务。
    command
        .spawn()
        .map_err(|error| format!("无法打开目录 {}：{error}", path.display()))?;

    Ok(())
}

/** 在后台刷新 SQLite/FTS5 索引，确保大知识库写索引时界面仍可响应。 */
pub(super) async fn index_snapshot_in_background(
    app: AppHandle,
    snapshot: &WorkspaceSnapshot,
) -> Result<(), String> {
    let index_app = app.clone();
    let index_snapshot = snapshot.clone();

    run_blocking("刷新本地检索索引", move || {
        storage::index_snapshot(&index_app, &index_snapshot)
    })
    .await
}

/** 回合结束后只 upsert 本轮会话，避免过期快照把其它会话删掉或打回旧模型/权限。 */
pub(super) async fn persist_turn_session(
    app: &AppHandle,
    snapshot: &WorkspaceSnapshot,
    session_id: &str,
) -> Result<(), String> {
    let persist_app = app.clone();
    let persist_snapshot = snapshot.clone();
    let persist_session_id = session_id.to_owned();

    run_blocking("保存 Agent 会话", move || {
        storage::save_snapshot_session(&persist_app, &persist_snapshot, &persist_session_id)
    })
    .await
}

/** 从新建文件相对路径提取初始标题，空白正文会在重扫时继续使用文件名。 */
pub(super) fn note_title_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名")
        .to_owned()
}

/** 从普通文档相对路径提取标题，txt/docx/pdf/图片首版都使用文件名 stem。 */
pub(super) fn document_title_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名文档")
        .to_owned()
}

/** 从目录相对路径取最后一级名称，用于 create_folder 后立即生成前端目录节点。 */
pub(super) fn folder_name_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名目录")
        .to_owned()
}

/** 修正活跃知识库、笔记和会话，避免扫描、移除后工作台指向不存在的对象。 */
pub(super) fn normalize_active_entities(
    snapshot: &mut WorkspaceSnapshot,
    preferred_knowledge_base_id: Option<&str>,
) {
    if snapshot.knowledge_bases.is_empty() {
        snapshot.active_knowledge_base_id.clear();
        snapshot.active_note_id.clear();
        snapshot.active_document_id.clear();
        snapshot.active_session_id.clear();
        return;
    }

    let active_knowledge_base_exists = snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == snapshot.active_knowledge_base_id);

    if !active_knowledge_base_exists {
        snapshot.active_knowledge_base_id = preferred_knowledge_base_id
            .and_then(|knowledge_base_id| {
                snapshot
                    .knowledge_bases
                    .iter()
                    .find(|knowledge_base| knowledge_base.id == knowledge_base_id)
                    .map(|knowledge_base| knowledge_base.id.clone())
            })
            .or_else(|| {
                snapshot
                    .knowledge_bases
                    .first()
                    .map(|knowledge_base| knowledge_base.id.clone())
            })
            .unwrap_or_default();
    }

    let active_note_exists = snapshot.notes.iter().any(|note| {
        note.id == snapshot.active_note_id
            && note.knowledge_base_id == snapshot.active_knowledge_base_id
    });
    let active_document_exists = snapshot.documents.iter().any(|document| {
        document.id == snapshot.active_document_id
            && document.knowledge_base_id == snapshot.active_knowledge_base_id
    });

    if active_document_exists {
        snapshot.active_note_id.clear();
    } else if !active_note_exists {
        snapshot.active_note_id = snapshot
            .notes
            .iter()
            .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
            .map(|note| note.id.clone())
            .unwrap_or_default();
    }

    if snapshot.active_note_id.is_empty() {
        if !active_document_exists {
            snapshot.active_document_id = snapshot
                .documents
                .iter()
                .find(|document| document.knowledge_base_id == snapshot.active_knowledge_base_id)
                .map(|document| document.id.clone())
                .unwrap_or_default();
        }
    } else {
        snapshot.active_document_id.clear();
    }

    if !snapshot
        .sessions
        .iter()
        .any(|session| session.id == snapshot.active_session_id)
    {
        snapshot.active_session_id = snapshot
            .sessions
            .iter()
            .find(|session| {
                session.knowledge_base_ids.iter().any(|knowledge_base_id| {
                    knowledge_base_id == &snapshot.active_knowledge_base_id
                })
            })
            .map(|session| session.id.clone())
            .unwrap_or_default();
    }
}
