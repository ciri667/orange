use crate::domain::{
    AgentSession, AgentSkill, AgentTurnPayload, AgentTurnResult, AppEventLog, ChangePayload,
    ClearDocumentHistoryPayload, CompactAgentContextPayload, CreateDocumentPayload,
    CreateFolderPayload, CreateNotePayload, DeleteAgentSkillPayload, DeleteDocumentPayload,
    DeleteKnowledgeBaseMemoryPayload, DeleteNotePayload, DeleteSessionPayload,
    DocumentHistoryEntry, DocumentHistoryEntryDetail, DocumentPreview, FeishuCredentialStatus,
    FeishuGatewayStatus, FolderEntry, ImGatewayStatus, ImIntegrationSettings,
    ImProviderCredentialStatus, ImProviderPayload, InstallAgentSkillPayload,
    InstallAgentSkillResult, KnowledgeBaseMemory, KnowledgeBaseSelection,
    LlmProviderModelRefreshResult, LoadAppEventLogsPayload, LoadDocumentHistoryEntryPayload,
    LoadDocumentHistoryPayload, LoadDocumentPreviewPayload, LoadSessionsPayload, ModelApiKeyStatus,
    ProposedChange, RefreshLlmProviderModelsPayload, RemoveKnowledgeBasePayload,
    RenameDocumentPayload, RenameNotePayload, RequestAuditLog, RescanKnowledgeBasePayload,
    RestoreDocumentHistoryEntryPayload, RestoreSessionContextPayload, SaveAgentSkillPayload,
    SaveDocumentContentPayload, SaveFeishuSecretPayload, SaveImProviderSecretPayload,
    SaveImSettingsPayload, SaveKnowledgeBaseMemoryPayload, SaveModelApiKeyPayload,
    SaveNoteContentPayload, SaveNoteImageAttachmentsPayload, SaveSessionPayload,
    SaveUserSettingsPayload, ScanKnowledgeBasePayload, ScanReport, ToggleAgentSkillPayload,
    UpdateSessionScopePayload, UserSettings, WorkspaceSnapshot, IM_PROVIDER_FEISHU,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::model_provider::{self, ProviderTemplate};
use crate::runtime;
use crate::skills;
use crate::storage;
use crate::text_edit::{replace_unique, UniqueReplacementError};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

/** 模型列表刷新最多保存的条目数，避免 OpenRouter 等聚合平台把设置 JSON 撑得过大。 */
const MAX_REFRESHED_LLM_MODELS: usize = 500;

/** 模型列表刷新超时时间；短于对话请求，保证设置页操作可快速失败重试。 */
const MODEL_LIST_HTTP_TIMEOUT_SECONDS: u64 = 20;

/** 加载工作台初始状态；从 SQLite 恢复已连接知识库并重新扫描真实支持文档。 */
#[tauri::command]
pub async fn load_workspace_state(app: AppHandle) -> Result<WorkspaceSnapshot, String> {
    let load_app = app.clone();
    let index_app = app.clone();
    let started_at = Instant::now();

    let snapshot = run_blocking("加载工作台状态", move || {
        storage::load_workspace_snapshot(&load_app)
    })
    .await?;
    let index_snapshot = snapshot.clone();

    allow_asset_protocol_for_knowledge_bases(&app, &snapshot)?;

    // 启动索引只影响后续检索，不阻塞首屏进入；失败时写 stderr 供桌面日志排查。
    tauri::async_runtime::spawn(async move {
        if let Err(error) = index_snapshot_in_background(index_app, &index_snapshot).await {
            log::warn!(target: "storage", "启动刷新本地检索索引失败：{error}");
        }
    });

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::App,
            "load_workspace_state",
            "completed",
            "已加载工作台状态。",
        )
        .duration(started_at.elapsed())
        .metadata(json!({
            "knowledgeBaseCount": snapshot.knowledge_bases.len(),
            "noteCount": snapshot.notes.len(),
            "documentCount": snapshot.documents.len(),
        })),
    );

    Ok(snapshot)
}

/** 读取文件系统真实修改时间；读取失败时记录脱敏日志并退回当前本地时间。 */
fn read_file_updated_at_or_now(
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

/** 文档历史目标上下文，只包含可写文件所需的脱敏定位信息。 */
#[derive(Clone, Debug)]
struct DocumentHistoryTargetContext {
    target_kind: String,
    entity_type: &'static str,
    entity_id: String,
    knowledge_base_id: String,
    relative_path: String,
    title: String,
    file_type: String,
}

/** 从快照解析历史记录目标；首版只允许 Markdown note 和 TXT document。 */
fn resolve_document_history_target(
    snapshot: &WorkspaceSnapshot,
    target_kind: &str,
    target_id: &str,
) -> Result<DocumentHistoryTargetContext, String> {
    match target_kind {
        "note" => {
            let note = snapshot
                .notes
                .iter()
                .find(|item| item.id == target_id)
                .ok_or_else(|| "找不到要查看历史的 Markdown 笔记。".to_owned())?;

            Ok(DocumentHistoryTargetContext {
                target_kind: "note".to_owned(),
                entity_type: "note",
                entity_id: note.id.clone(),
                knowledge_base_id: note.knowledge_base_id.clone(),
                relative_path: note.path.clone(),
                title: note.title.clone(),
                file_type: "markdown".to_owned(),
            })
        }
        "document" => {
            let document = snapshot
                .documents
                .iter()
                .find(|item| item.id == target_id)
                .ok_or_else(|| "找不到要查看历史的文档。".to_owned())?;

            if document.file_type != "txt" {
                return Err("只有 TXT 文档支持历史记录。".to_owned());
            }

            Ok(DocumentHistoryTargetContext {
                target_kind: "document".to_owned(),
                entity_type: "document",
                entity_id: document.id.clone(),
                knowledge_base_id: document.knowledge_base_id.clone(),
                relative_path: document.path.clone(),
                title: document.title.clone(),
                file_type: "txt".to_owned(),
            })
        }
        _ => Err("该文件类型暂不支持历史记录。".to_owned()),
    }
}

/** 覆盖写入前捕获当前磁盘版本；失败会阻止后续写入，避免没有回档点。 */
async fn capture_document_history_before_write(
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
async fn clear_document_history_after_delete_best_effort(
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

/** 读取持久化 Agent 会话，并按当前工作台快照清理已失效的知识库或笔记引用。 */
#[tauri::command]
pub async fn load_sessions(
    app: AppHandle,
    payload: LoadSessionsPayload,
) -> Result<Vec<AgentSession>, String> {
    run_blocking("读取 Agent 会话", move || {
        let mut sessions = storage::load_sessions_for_snapshot(&app, &payload.snapshot)?;
        let migrated_sessions = storage::migrate_legacy_im_session_identities(&app, &mut sessions)?;

        if !migrated_sessions.is_empty() {
            storage::save_session_records(&app, &migrated_sessions)?;
            for session in migrated_sessions {
                if let Some(identity) = session.im_identity {
                    logging::write_app_event_best_effort(
                        &app,
                        build_im_identity_event(&session.id, &identity, "migrated"),
                    );
                }
            }
        }

        Ok(sessions)
    })
    .await
}

/** 保存单个 Agent 会话，供前端创建会话或更新消息后统一进入 SQLite。 */
#[tauri::command]
pub async fn save_session(
    app: AppHandle,
    payload: SaveSessionPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("保存 Agent 会话", move || {
        storage::save_session(&app, payload.snapshot, payload.session)
    })
    .await
}

/** 逻辑删除单个 Agent 会话；记录保留在 SQLite payload 中但不再进入普通会话列表。 */
#[tauri::command]
pub async fn delete_session(
    app: AppHandle,
    payload: DeleteSessionPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("删除 Agent 会话", move || {
        storage::delete_session(&app, payload.snapshot, &payload.session_id)
    })
    .await
}

/** 更新当前会话工具范围；后端强制保留激活知识库并剔除不存在的引用。 */
#[tauri::command]
pub async fn update_session_scope(
    app: AppHandle,
    payload: UpdateSessionScopePayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("更新 Agent 会话范围", move || {
        storage::update_session_scope(
            &app,
            payload.snapshot,
            &payload.session_id,
            payload.knowledge_base_ids,
            &payload.active_knowledge_base_id,
        )
    })
    .await
}

/** 从历史会话恢复知识库、笔记和会话焦点。 */
#[tauri::command]
pub async fn restore_session_context(
    app: AppHandle,
    payload: RestoreSessionContextPayload,
) -> Result<WorkspaceSnapshot, String> {
    run_blocking("恢复 Agent 会话上下文", move || {
        storage::restore_session_context(&app, payload.snapshot, &payload.session_id)
    })
    .await
}

/** 读取用户模型和隐私设置，缺失时返回本地安全默认值。 */
#[tauri::command]
pub async fn load_user_settings(app: AppHandle) -> Result<UserSettings, String> {
    run_blocking("读取用户设置", move || {
        storage::load_user_settings(&app)
    })
    .await
}

/** 保存用户模型和隐私设置；明文 API key 不进入这份配置。 */
#[tauri::command]
pub async fn save_user_settings(
    app: AppHandle,
    payload: SaveUserSettingsPayload,
) -> Result<UserSettings, String> {
    let saved_settings = payload.settings;
    let settings_app = app.clone();
    let started_at = Instant::now();
    let model_enabled = saved_settings.model_config.enabled;
    let provider_count = saved_settings.model_config.providers.len();
    let default_provider_id = saved_settings.model_config.default_provider_id.clone();

    let result = run_blocking("保存用户设置", move || {
        // 返回值使用归一化后的设置（key_reference 已按 providerId 重新计算），
        // 避免前端状态和实际持久化、keyring 写入位置出现分歧。
        storage::save_user_settings(&settings_app, &saved_settings)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Settings,
                "save_user_settings",
                "completed",
                "已保存用户设置。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "modelEnabled": model_enabled,
                "providerCount": provider_count,
                "defaultProviderId": default_provider_id,
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Settings,
                "save_user_settings",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 读取全部知识库的跨会话记忆，供设置页列表展示；记录脱敏审计事件。 */
#[tauri::command]
pub async fn load_knowledge_base_memories(
    app: AppHandle,
) -> Result<Vec<KnowledgeBaseMemory>, String> {
    let started_at = Instant::now();
    let load_app = app.clone();
    let result = run_blocking("读取跨会话记忆", move || {
        storage::load_knowledge_base_memories(&load_app)
    })
    .await;

    match &result {
        Ok(memories) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Agent,
                "load_knowledge_base_memories",
                "completed",
                "已读取跨会话记忆。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "memoryCount": memories.len() })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "load_knowledge_base_memories",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 保存单个知识库的跨会话记忆；写入前归一化并做敏感信息脱敏，返回脱敏后的 memory。
 * metadata 只记录 enabled、entryCount 和 knowledgeBaseId 是否非空，不暴露正文。 */
#[tauri::command]
pub async fn save_knowledge_base_memory(
    app: AppHandle,
    payload: SaveKnowledgeBaseMemoryPayload,
) -> Result<KnowledgeBaseMemory, String> {
    let started_at = Instant::now();
    let knowledge_base_id = payload.knowledge_base_id.clone();
    let memory_enabled = payload.memory.enabled;
    let memory_entry_count = payload.memory.entries.len();

    let save_app = app.clone();
    let result = run_blocking("保存跨会话记忆", move || {
        storage::save_knowledge_base_memory(&save_app, &payload.knowledge_base_id, payload.memory)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Agent,
                "save_knowledge_base_memory",
                "completed",
                "已保存跨会话记忆。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "knowledgeBaseIdPresent": !knowledge_base_id.is_empty(),
                "enabled": memory_enabled,
                "entryCount": memory_entry_count,
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "save_knowledge_base_memory",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 删除单个知识库的跨会话记忆；metadata 只记录知识库 ID 是否非空。 */
#[tauri::command]
pub async fn delete_knowledge_base_memory(
    app: AppHandle,
    payload: DeleteKnowledgeBaseMemoryPayload,
) -> Result<(), String> {
    let started_at = Instant::now();
    let knowledge_base_id = payload.knowledge_base_id.clone();

    let delete_app = app.clone();
    let result = run_blocking("删除跨会话记忆", move || {
        storage::delete_knowledge_base_memory(&delete_app, &payload.knowledge_base_id)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Agent,
                "delete_knowledge_base_memory",
                "completed",
                "已删除跨会话记忆。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "knowledgeBaseIdPresent": !knowledge_base_id.is_empty() })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "delete_knowledge_base_memory",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 读取即时通讯集成设置；敏感凭证只返回 keyring 状态不返回明文。 */
#[tauri::command]
pub async fn load_im_settings(app: AppHandle) -> Result<ImIntegrationSettings, String> {
    run_blocking("读取即时通讯设置", move || {
        storage::load_im_settings(&app)
    })
    .await
}
/** 保存即时通讯集成设置；飞书 appSecret 必须走独立 keyring 命令。 */
#[tauri::command]
pub async fn save_im_settings(
    app: AppHandle,
    payload: SaveImSettingsPayload,
) -> Result<ImIntegrationSettings, String> {
    let settings_app = app.clone();
    let started_at = Instant::now();
    let result = run_blocking("保存即时通讯设置", move || {
        storage::save_im_settings(&settings_app, &payload.settings)
    })
    .await;

    match &result {
        Ok(settings) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "save_im_settings",
                "completed",
                "已保存即时通讯设置。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerCount": settings.providers.len(),
                "enabledProviderCount": settings.providers.iter().filter(|provider| provider.enabled).count(),
                "providers": settings.providers.iter().map(|provider| json!({
                    "providerId": provider.provider_id,
                    "enabled": provider.enabled,
                    "knowledgeBaseCount": provider.default_knowledge_base_ids.len(),
                    "allowedUserCount": provider.allowed_user_open_ids.len(),
                    "allowedChatCount": provider.allowed_chat_ids.len(),
                    "requireMention": provider.require_mention,
                })).collect::<Vec<_>>(),
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "save_im_settings",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 保存 IM provider secret 到系统安全存储；命令日志不包含 secret 明文。 */
#[tauri::command]
pub async fn save_im_provider_secret(
    app: AppHandle,
    payload: SaveImProviderSecretPayload,
) -> Result<ImProviderCredentialStatus, String> {
    let provider_id = payload.provider_id.trim().to_ascii_lowercase();
    let started_at = Instant::now();
    let result_provider_id = provider_id.clone();
    let result = run_blocking("保存 IM provider secret", move || {
        storage::save_im_provider_secret(&provider_id, &payload.secret)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "save_im_provider_secret",
                "completed",
                "IM provider secret 已保存。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "providerId": result_provider_id })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "save_im_provider_secret",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "providerId": result_provider_id })),
        ),
    }

    result
}

/** 读取 IM provider secret 是否已配置；不返回明文 secret。 */
#[tauri::command]
pub async fn load_im_provider_credential_status(
    payload: ImProviderPayload,
) -> Result<ImProviderCredentialStatus, String> {
    let provider_id = payload.provider_id.trim().to_ascii_lowercase();

    run_blocking("读取 IM provider 凭证状态", move || {
        storage::load_im_provider_credential_status(&provider_id)
    })
    .await
}

/** 启动 IM provider 网关，消息处理进入后台任务。 */
#[tauri::command]
pub async fn start_im_gateway(
    app: AppHandle,
    payload: ImProviderPayload,
) -> Result<ImGatewayStatus, String> {
    let provider_id = payload.provider_id.trim().to_ascii_lowercase();

    crate::im::start_gateway(app, &provider_id).await
}

/** 停止 IM provider 网关，不清空任何配置或凭证。 */
#[tauri::command]
pub async fn stop_im_gateway(
    app: AppHandle,
    payload: ImProviderPayload,
) -> Result<ImGatewayStatus, String> {
    let provider_id = payload.provider_id.trim().to_ascii_lowercase();

    run_blocking("停止 IM provider 网关", move || {
        crate::im::stop_gateway(&app, &provider_id)
    })
    .await
}

/** 读取 IM provider 网关运行态。 */
#[tauri::command]
pub async fn load_im_gateway_status(
    app: AppHandle,
    payload: ImProviderPayload,
) -> Result<ImGatewayStatus, String> {
    let provider_id = payload.provider_id.trim().to_ascii_lowercase();

    run_blocking("读取 IM provider 网关状态", move || {
        crate::im::load_gateway_status(&app, &provider_id)
    })
    .await
}

/** 保存飞书 appSecret 到系统安全存储；兼容旧命令，内部转发到通用 provider 命令。 */
#[tauri::command]
pub async fn save_feishu_app_secret(
    app: AppHandle,
    payload: SaveFeishuSecretPayload,
) -> Result<FeishuCredentialStatus, String> {
    let started_at = Instant::now();
    let result = run_blocking("保存飞书 appSecret", move || {
        storage::save_im_provider_secret(IM_PROVIDER_FEISHU, &payload.app_secret)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Im,
                "save_feishu_app_secret",
                "completed",
                "飞书 appSecret 已保存。",
            )
            .duration(started_at.elapsed()),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Im,
                "save_feishu_app_secret",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 读取飞书 appSecret 是否已配置；不返回明文 secret。 */
#[tauri::command]
pub async fn load_feishu_credential_status() -> Result<FeishuCredentialStatus, String> {
    run_blocking("读取飞书凭证状态", || {
        storage::load_im_provider_credential_status(IM_PROVIDER_FEISHU)
    })
    .await
}

/** 启动飞书长连接网关；兼容旧命令，内部转发到通用 provider 路由。 */
#[tauri::command]
pub async fn start_feishu_gateway(app: AppHandle) -> Result<FeishuGatewayStatus, String> {
    crate::im::start_gateway(app, IM_PROVIDER_FEISHU).await
}

/** 停止飞书长连接网关；兼容旧命令，不清空任何配置或凭证。 */
#[tauri::command]
pub async fn stop_feishu_gateway(app: AppHandle) -> Result<FeishuGatewayStatus, String> {
    run_blocking("停止飞书长连接网关", move || {
        crate::im::stop_gateway(&app, IM_PROVIDER_FEISHU)
    })
    .await
}

/** 读取飞书长连接网关运行态；兼容旧命令。 */
#[tauri::command]
pub async fn load_feishu_gateway_status(app: AppHandle) -> Result<FeishuGatewayStatus, String> {
    run_blocking("读取飞书网关状态", move || {
        crate::im::load_gateway_status(&app, IM_PROVIDER_FEISHU)
    })
    .await
}

/** 读取内置和用户自建 skills，内置定义会合并用户保存的启停偏好。 */
#[tauri::command]
pub async fn load_agent_skills(app: AppHandle) -> Result<Vec<AgentSkill>, String> {
    run_blocking("读取 Skills", move || {
        let connection = storage::open_database(&app)?;

        skills::load_agent_skills(&app, &connection)
    })
    .await
}

/** 打开橘记 用户 Skills 文件夹，浏览器开发态由前端 mock 只展示路径。 */
#[tauri::command]
pub async fn open_user_skills_folder(app: AppHandle) -> Result<String, String> {
    let skills_app = app.clone();
    let started_at = Instant::now();
    let result = run_blocking("打开用户 Skills 文件夹", move || {
        let skills_root = skills::user_skills_root(&skills_app)?;

        open_folder_in_system(&skills_root)?;

        Ok(skills_root.to_string_lossy().to_string())
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "open_user_skills_folder",
                "completed",
                "已打开用户 Skills 文件夹。",
            )
            .duration(started_at.elapsed()),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "open_user_skills_folder",
                "failed",
                error,
            )
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 新增或编辑用户自建 skill；内置 skill 只能通过启停入口修改状态。 */
#[tauri::command]
pub async fn save_agent_skill(
    app: AppHandle,
    payload: SaveAgentSkillPayload,
) -> Result<AgentSkill, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill.id.clone();
    let skill_name = payload.skill.name.clone();
    let started_at = Instant::now();
    let result = run_blocking("保存 Skill", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::save_user_skill(&skills_app, &connection, payload.skill)
    })
    .await;

    match &result {
        Ok(saved_skill) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "save_agent_skill",
                "completed",
                "已保存 Skill。",
            )
            .duration(started_at.elapsed())
            .entity("skill", saved_skill.id.clone())
            .metadata(
                json!({ "name": saved_skill.name.clone(), "source": saved_skill.source.clone() }),
            ),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "save_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id)
            .metadata(json!({ "name": skill_name })),
        ),
    }

    result
}

/** 启停 skill；启用的 skill 会以名称和描述进入 Agent system prompt。 */
#[tauri::command]
pub async fn toggle_agent_skill(
    app: AppHandle,
    payload: ToggleAgentSkillPayload,
) -> Result<AgentSkill, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill_id.clone();
    let enabled = payload.enabled;
    let started_at = Instant::now();
    let result = run_blocking("更新 Skill 状态", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::toggle_agent_skill(&skills_app, &connection, &payload.skill_id, payload.enabled)
    })
    .await;

    match &result {
        Ok(skill) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "toggle_agent_skill",
                "completed",
                "已更新 Skill 状态。",
            )
            .duration(started_at.elapsed())
            .entity("skill", skill.id.clone())
            .metadata(json!({ "enabled": skill.enabled })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "toggle_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id)
            .metadata(json!({ "enabled": enabled })),
        ),
    }

    result
}

/** 删除用户自建 skill；内置 skill 必须保留供用户重新启用。 */
#[tauri::command]
pub async fn delete_agent_skill(
    app: AppHandle,
    payload: DeleteAgentSkillPayload,
) -> Result<Vec<AgentSkill>, String> {
    let skills_app = app.clone();
    let skill_id = payload.skill_id.clone();
    let started_at = Instant::now();
    let result = run_blocking("删除 Skill", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::delete_user_skill(&skills_app, &connection, &payload.skill_id)?;
        skills::load_agent_skills(&skills_app, &connection)
    })
    .await;

    match &result {
        Ok(_) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "delete_agent_skill",
                "completed",
                "已删除用户 Skill。",
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id.clone()),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "delete_agent_skill",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .entity("skill", skill_id),
        ),
    }

    result
}

/** 安装第三方 Skill 包；默认停用，用户审阅后再手动启用。 */
#[tauri::command]
pub async fn install_agent_skill(
    app: AppHandle,
    payload: InstallAgentSkillPayload,
) -> Result<InstallAgentSkillResult, String> {
    let source_type = payload.source_type.clone();
    let conflict_strategy = payload.conflict_strategy.clone();
    let enable_after_install = payload.enable_after_install;
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Skill,
            "install_agent_skill",
            "started",
            "开始安装第三方 Skill。",
        )
        .operation_id(operation_id.clone())
        .metadata(json!({
            "sourceType": source_type.clone(),
            "conflictStrategy": conflict_strategy.clone(),
            "enableAfterInstall": enable_after_install,
        })),
    );

    let prepare_result = prepare_skill_install_source(&app, &payload).await;
    let result = match prepare_result {
        Ok(prepared_source) => {
            let install_app = app.clone();
            let install_source_type = source_type.clone();
            let install_conflict_strategy = conflict_strategy.clone();
            let install_enable_after_install = enable_after_install;

            run_blocking("安装 Skill", move || {
                let connection = storage::open_database(&install_app)?;
                let skills_root = skills::user_skills_root(&install_app)?;

                skills::install_agent_skills_from_prepared_root(
                    &connection,
                    &skills_root,
                    prepared_source.root_path(),
                    skills::SkillInstallOptions {
                        source_type: install_source_type,
                        source_summary: prepared_source.source_summary().to_owned(),
                        enable_after_install: install_enable_after_install,
                        conflict_strategy: install_conflict_strategy,
                    },
                )
            })
            .await
        }
        Err(error) => Err(error),
    };

    match &result {
        Ok(install_result) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Skill,
                "install_agent_skill",
                "completed",
                "已安装第三方 Skill。",
            )
            .operation_id(operation_id)
            .duration(started_at.elapsed())
            .metadata(json!({
                "sourceType": install_result.source_type.clone(),
                "sourceSummary": install_result.source_summary.clone(),
                "installedCount": install_result.installed_count,
                "fileCount": install_result.file_count,
                "warningCount": install_result.warnings.len(),
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Skill,
                "install_agent_skill",
                "failed",
                error,
            )
            .operation_id(operation_id)
            .duration(started_at.elapsed())
            .metadata(json!({
                "sourceType": source_type,
                "conflictStrategy": conflict_strategy,
                "enableAfterInstall": enable_after_install,
            })),
        ),
    }

    result
}

/** 保存 BYOK 模型密钥到系统安全存储，按 providerId 隔离，SQLite 只保存 keyReference。 */
#[tauri::command]
pub async fn save_model_api_key(
    app: AppHandle,
    payload: SaveModelApiKeyPayload,
) -> Result<ModelApiKeyStatus, String> {
    let started_at = Instant::now();
    let provider_id = payload.provider_id.clone();
    let result = run_blocking("保存模型密钥", move || {
        storage::save_model_api_key(&payload.provider_id, &payload.api_key)
    })
    .await;

    match &result {
        Ok(status) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Security,
                "save_model_api_key",
                "completed",
                "已更新模型密钥状态。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": status.provider_id.clone(),
                "configured": status.configured,
                "keyReference": status.key_reference.clone(),
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Security,
                "save_model_api_key",
                "failed",
                error,
            )
            .duration(started_at.elapsed())
            .metadata(json!({ "providerId": provider_id })),
        ),
    }

    result
}

/** 批量读取每个 provider 的 BYOK 模型密钥状态，只返回是否已配置，不返回明文。 */
#[tauri::command]
pub async fn load_model_api_key_statuses(app: AppHandle) -> Result<Vec<ModelApiKeyStatus>, String> {
    run_blocking("读取模型密钥状态", move || {
        let settings = storage::load_user_settings(&app)?;

        storage::load_model_api_key_statuses(&settings.model_config.providers)
    })
    .await
}

/** 读取内置 LLM Provider 模板，供设置页“新增 Provider”入口预填参数。 */
#[tauri::command]
pub async fn load_llm_provider_templates() -> Result<Vec<ProviderTemplate>, String> {
    Ok(model_provider::provider_templates())
}

/** 刷新指定 OpenAI-compatible provider 的可用模型列表，并把启用状态合并回用户设置。 */
#[tauri::command]
pub async fn refresh_llm_provider_models(
    app: AppHandle,
    payload: RefreshLlmProviderModelsPayload,
) -> Result<LlmProviderModelRefreshResult, String> {
    let provider_id = payload.provider_id.trim().to_owned();
    let started_at = Instant::now();
    let mut endpoint_host_for_log = "unknown-host".to_owned();
    let result = async {
        let load_app = app.clone();
        let provider_id_for_load = provider_id.clone();
        let (mut settings, provider, api_key) =
            run_blocking("读取模型 provider 设置", move || {
                let settings = storage::load_user_settings(&load_app)?;
                let provider = settings
                    .model_config
                    .providers
                    .iter()
                    .find(|provider| provider.id == provider_id_for_load)
                    .cloned()
                    .ok_or_else(|| format!("未找到 Provider 配置：{provider_id_for_load}"))?;
                let api_key = if provider.requires_api_key {
                    storage::load_model_api_key(&provider.key_reference)?.ok_or_else(|| {
                        format!(
                            "Provider「{}」未找到模型密钥。请先保存 API key 后再获取模型列表。",
                            provider.name
                        )
                    })?
                } else {
                    String::new()
                };

                Ok::<_, String>((settings, provider, api_key))
            })
            .await?;
        let endpoint = model_provider::models_endpoint(&provider.api_base);
        endpoint_host_for_log = model_provider::endpoint_host(&endpoint);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(MODEL_LIST_HTTP_TIMEOUT_SECONDS))
            .build()
            .map_err(|error| format!("无法创建模型列表 HTTP client：{error}"))?;
        let response_body = match send_model_list_request(&client, &endpoint, &api_key).await {
            Ok(body) => body,
            Err(error) => {
                if let Some(ollama_endpoint) =
                    model_provider::ollama_tags_endpoint(&provider.api_base)
                {
                    send_model_list_request(&client, &ollama_endpoint, &api_key)
                        .await
                        .map_err(|fallback_error| {
                            model_provider::redact_model_error_text(&format!(
                                "{error}；Ollama fallback 也失败：{fallback_error}"
                            ))
                        })?
                } else {
                    return Err(error);
                }
            }
        };
        let fetched_at = storage::format_local_datetime();
        let mut discovered_models =
            model_provider::parse_provider_models_response(&response_body, &fetched_at)?;

        discovered_models.truncate(MAX_REFRESHED_LLM_MODELS);
        let fetched_count = discovered_models.len();
        let provider_for_save = settings
            .model_config
            .providers
            .iter_mut()
            .find(|candidate| candidate.id == provider_id)
            .ok_or_else(|| format!("未找到 Provider 配置：{provider_id}"))?;

        model_provider::merge_discovered_models(provider_for_save, discovered_models, &fetched_at);

        let model_count = provider_for_save.models.len();
        let enabled_count = provider_for_save
            .models
            .iter()
            .filter(|model| model.enabled)
            .count();
        let save_app = app.clone();
        let saved_settings = run_blocking("保存刷新后的模型列表", move || {
            storage::save_user_settings(&save_app, &settings)
        })
        .await?;

        Ok::<_, String>(LlmProviderModelRefreshResult {
            settings: saved_settings,
            provider_id: provider_id.clone(),
            fetched_at: fetched_at.clone(),
            fetched_count,
            model_count,
            enabled_count,
            message: format!("已获取 {fetched_count} 个模型，当前启用 {enabled_count} 个。"),
        })
    }
    .await;

    match &result {
        Ok(result) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Model,
                "refresh_llm_provider_models",
                "completed",
                "已刷新模型列表。",
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": result.provider_id,
                "endpointHost": endpoint_host_for_log.clone(),
                "fetchedCount": result.fetched_count,
                "modelCount": result.model_count,
                "enabledCount": result.enabled_count,
            })),
        ),
        Err(error) => logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Model,
                "refresh_llm_provider_models",
                "failed",
                model_provider::redact_model_error_text(error),
            )
            .duration(started_at.elapsed())
            .metadata(json!({
                "providerId": provider_id,
                "endpointHost": endpoint_host_for_log.clone(),
            })),
        ),
    }

    result
}

/** 发送模型列表请求；只返回响应正文，错误信息会脱敏并限制长度。 */
async fn send_model_list_request(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
) -> Result<String, String> {
    let mut request_builder = client.get(endpoint);

    if !api_key.trim().is_empty() {
        request_builder = request_builder.bearer_auth(api_key);
    }

    let response = request_builder.send().await.map_err(|error| {
        model_provider::redact_model_error_text(&format!("无法发送模型列表请求：{error}"))
    })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取模型列表响应：{error}"))?;

    if !status.is_success() {
        return Err(model_provider::redact_model_error_text(&format!(
            "模型列表请求失败：HTTP {status} {body}"
        )));
    }

    Ok(body)
}

/** 读取最近模型请求和工具调用审计摘要，用于设置页解释发送边界。 */
#[tauri::command]
pub async fn load_request_audit_logs(app: AppHandle) -> Result<Vec<RequestAuditLog>, String> {
    run_blocking("读取请求审计日志", move || {
        storage::load_request_audit_logs(&app, 20)
    })
    .await
}

/** 读取最近应用事件日志，用于设置页展示运行诊断和用户关键操作。 */
#[tauri::command]
pub async fn load_app_event_logs(
    app: AppHandle,
    payload: LoadAppEventLogsPayload,
) -> Result<Vec<AppEventLog>, String> {
    run_blocking("读取应用事件日志", move || {
        storage::load_app_event_logs(
            &app,
            payload.limit.unwrap_or(100),
            payload.level.as_deref(),
            payload.category.as_deref(),
        )
    })
    .await
}

/** 清空用户可读应用事件日志，不删除 Tauri 文件诊断日志。 */
#[tauri::command]
pub async fn clear_app_event_logs(app: AppHandle) -> Result<(), String> {
    let event_app = app.clone();

    run_blocking("清空应用事件日志", move || {
        storage::clear_app_event_logs(&app)
    })
    .await?;

    logging::write_app_event_best_effort(
        &event_app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Settings,
            "clear_app_event_logs",
            "completed",
            "已清空应用事件日志。",
        ),
    );

    Ok(())
}

/** 打开系统应用日志目录，便于用户附带文件日志排查桌面端问题。 */
#[tauri::command]
pub async fn open_app_log_folder(app: AppHandle) -> Result<String, String> {
    let event_app = app.clone();

    let log_dir = run_blocking("打开应用日志目录", move || {
        let log_dir = logging::app_log_dir(&app)?;

        fs::create_dir_all(&log_dir).map_err(|error| format!("无法创建应用日志目录：{error}"))?;
        open_folder_in_system(&log_dir)?;

        Ok(log_dir)
    })
    .await?;
    let display_path = log_dir.to_string_lossy().to_string();

    logging::write_app_event_best_effort(
        &event_app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Settings,
            "open_app_log_folder",
            "completed",
            "已打开应用日志目录。",
        ),
    );

    Ok(display_path)
}

/** 打开系统目录选择器，让用户连接一个本地支持文档知识库。 */
#[tauri::command]
pub async fn select_knowledge_base(app: AppHandle) -> Result<KnowledgeBaseSelection, String> {
    let started_at = Instant::now();
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择支持文档知识库目录")
        .pick_folder(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    let selected_path = receiver
        .recv()
        .await
        .flatten()
        .ok_or_else(|| "未选择知识库目录".to_owned())?;
    let path = selected_path
        .as_path()
        .ok_or_else(|| "无法读取所选目录路径".to_owned())?
        .to_path_buf();
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("本地知识库")
        .to_owned();
    let count_path = path.clone();
    let note_count =
        tauri::async_runtime::spawn_blocking(move || count_markdown_files(&count_path))
            .await
            .map_err(|error| format!("统计 Markdown 文件时后台任务失败：{error}"))??;

    let selection = KnowledgeBaseSelection {
        id: storage::create_id("kb"),
        name,
        path: path.to_string_lossy().to_string(),
        note_count,
    };

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "select_knowledge_base",
            "completed",
            "已选择知识库目录。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(selection.id.clone())
        .metadata(json!({ "noteCount": selection.note_count })),
    );

    Ok(selection)
}

/** 扫描用户选择的支持文档目录，并合并进当前工作台快照。 */
#[tauri::command]
pub async fn scan_knowledge_base(
    app: AppHandle,
    payload: ScanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let selection = payload.selection;
    let selected_knowledge_base_id = selection.id.clone();
    let (knowledge_base, folders, notes, documents) =
        run_blocking("扫描支持文档知识库", move || {
            storage::scan_supported_documents_directory(&selection)
        })
        .await?;
    let knowledge_base_id = knowledge_base.id.clone();

    allow_asset_protocol_directory(&app, Path::new(&knowledge_base.path))?;

    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = notes
        .first()
        .map(|note| note.id.clone())
        .unwrap_or_default();
    snapshot.active_document_id = if snapshot.active_note_id.is_empty() {
        documents
            .first()
            .map(|document| document.id.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    snapshot.knowledge_bases.push(knowledge_base);
    snapshot.folders.extend(folders);
    snapshot.notes.extend(notes);
    snapshot.documents.extend(documents);
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&knowledge_base_id));

    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "scan_knowledge_base",
            "completed",
            "已连接并扫描知识库。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .metadata(json!({
            "folderCount": snapshot.folders.iter().filter(|folder| folder.knowledge_base_id == selected_knowledge_base_id).count(),
            "noteCount": snapshot.notes.iter().filter(|note| note.knowledge_base_id == selected_knowledge_base_id).count(),
            "documentCount": snapshot.documents.iter().filter(|document| document.knowledge_base_id == selected_knowledge_base_id).count(),
        })),
    );

    Ok(snapshot)
}

/** 重新扫描一个已连接知识库，用真实支持文档替换该知识库的缓存条目。 */
#[tauri::command]
pub async fn rescan_knowledge_base(
    app: AppHandle,
    payload: RescanKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let requested_knowledge_base_id = payload.knowledge_base_id.clone();
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要重新扫描的知识库".to_owned())?;
    let previous_knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let selection = KnowledgeBaseSelection {
        id: previous_knowledge_base.id.clone(),
        name: previous_knowledge_base.name.clone(),
        path: previous_knowledge_base.path.clone(),
        note_count: previous_knowledge_base.note_count,
    };
    let previous_active_note_id = snapshot.active_note_id.clone();
    let previous_active_document_id = snapshot.active_document_id.clone();
    let scan_result = run_blocking("重新扫描支持文档知识库", move || {
        storage::scan_supported_documents_directory(&selection)
    })
    .await;
    let (mut rescanned_knowledge_base, rescanned_folders, rescanned_notes, rescanned_documents) =
        match scan_result {
            Ok(result) => result,
            Err(error) => {
                let error_message = format!("无法访问已连接目录：{error}");
                let mut failed_knowledge_base = previous_knowledge_base;

                failed_knowledge_base.status = "error".to_owned();
                failed_knowledge_base.description = error_message.clone();
                failed_knowledge_base.note_count = 0;
                failed_knowledge_base.document_count = 0;
                failed_knowledge_base.updated_at = "刚刚".to_owned();
                failed_knowledge_base.scan_report = Some(ScanReport {
                    scanned_file_count: 0,
                    scanned_by_type: crate::domain::default_scanned_by_type(),
                    failed_file_count: 1,
                    skipped_directories: Vec::new(),
                    errors: vec![error_message.clone()],
                });
                snapshot.knowledge_bases[knowledge_base_index] = failed_knowledge_base;
                snapshot
                    .notes
                    .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
                snapshot
                    .folders
                    .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
                snapshot
                    .documents
                    .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);
                normalize_sessions_after_rescan(&mut snapshot, &payload.knowledge_base_id);
                normalize_knowledge_base_flags(&mut snapshot);
                normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));
                index_snapshot_in_background(app.clone(), &snapshot).await?;

                logging::write_app_event_best_effort(
                    &app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::KnowledgeBase,
                        "rescan_knowledge_base",
                        "failed",
                        error_message,
                    )
                    .duration(started_at.elapsed())
                    .knowledge_base_id(requested_knowledge_base_id.clone()),
                );

                return Ok(snapshot);
            }
        };

    rescanned_knowledge_base.semantic_index_enabled =
        previous_knowledge_base.semantic_index_enabled;
    rescanned_knowledge_base.is_default = previous_knowledge_base.is_default;
    rescanned_knowledge_base.updated_at = "刚刚".to_owned();
    rescanned_knowledge_base.note_count = rescanned_notes.len();
    rescanned_knowledge_base.document_count = rescanned_documents.len();
    allow_asset_protocol_directory(&app, Path::new(&rescanned_knowledge_base.path))?;
    snapshot.knowledge_bases[knowledge_base_index] = rescanned_knowledge_base.clone();

    // 重扫只替换目标知识库的文件条目，其他知识库和会话消息保持不变。
    snapshot
        .notes
        .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .folders
        .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .documents
        .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);
    snapshot.folders.extend(rescanned_folders);
    snapshot.notes.extend(rescanned_notes);
    snapshot.documents.extend(rescanned_documents);
    normalize_sessions_after_rescan(&mut snapshot, &payload.knowledge_base_id);

    if snapshot.active_knowledge_base_id == payload.knowledge_base_id {
        snapshot.active_note_id = snapshot
            .notes
            .iter()
            .find(|note| note.id == previous_active_note_id)
            .or_else(|| {
                snapshot
                    .notes
                    .iter()
                    .find(|note| note.knowledge_base_id == payload.knowledge_base_id)
            })
            .map(|note| note.id.clone())
            .unwrap_or_default();
        snapshot.active_document_id = if snapshot.active_note_id.is_empty() {
            snapshot
                .documents
                .iter()
                .find(|document| document.id == previous_active_document_id)
                .or_else(|| {
                    snapshot
                        .documents
                        .iter()
                        .find(|document| document.knowledge_base_id == payload.knowledge_base_id)
                })
                .map(|document| document.id.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
    }
    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, Some(&payload.knowledge_base_id));

    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "rescan_knowledge_base",
            "completed",
            "已重新扫描知识库。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(requested_knowledge_base_id)
        .metadata(json!({
            "noteCount": rescanned_knowledge_base.note_count,
            "documentCount": rescanned_knowledge_base.document_count,
        })),
    );

    Ok(snapshot)
}

/** 将当前已连接知识库目录加入 Tauri asset 协议 scope，供 Markdown 预览加载本地图片。 */
fn allow_asset_protocol_for_knowledge_bases(
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
fn allow_asset_protocol_directory(app: &AppHandle, path: &Path) -> Result<(), String> {
    app.asset_protocol_scope()
        .allow_directory(path, true)
        .map_err(|error| format!("无法授权 Markdown 图片预览目录 {}：{error}", path.display()))
}

/** 用户主动新建空白 Markdown，直接落盘并打开为当前可编辑笔记。 */
#[tauri::command]
pub async fn create_note(
    app: AppHandle,
    payload: CreateNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建笔记的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建笔记。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path.unwrap_or_default();
    let file_name = payload.file_name;
    let relative_path = run_blocking("创建空白 Markdown 文件", move || {
        storage::create_blank_markdown_file(&root_path, &parent_path, file_name.as_deref())
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let note_id = storage::create_stable_note_id(&knowledge_base.id, &relative_path);
    let created_note_path = PathBuf::from(&knowledge_base.path).join(&relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "create_note",
        &knowledge_base.id,
        "note",
        &note_id,
        &relative_path,
        &created_note_path,
    );
    let new_note = crate::domain::Note {
        id: note_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: note_title_from_path(&relative_path),
        path: relative_path,
        content: String::new(),
        tags: Vec::new(),
        updated_at,
        backlinks: Vec::new(),
        content_hash: storage::hash_content(""),
    };

    snapshot.notes.insert(0, new_note);
    snapshot.knowledge_bases[knowledge_base_index].note_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
        *scan_report
            .scanned_by_type
            .entry("markdown".to_owned())
            .or_insert(0) += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id = note_id;
    snapshot.active_document_id.clear();
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_note",
            "completed",
            "已创建 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", snapshot.active_note_id.clone())
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 用户主动新建空白 txt 文档，直接落盘并打开为当前普通文档。 */
#[tauri::command]
pub async fn create_document(
    app: AppHandle,
    payload: CreateDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建文档的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建 TXT 文档。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path.unwrap_or_default();
    let file_name = payload.file_name;
    let relative_path = run_blocking("创建空白 TXT 文件", move || {
        storage::create_blank_text_document_file(&root_path, &parent_path, file_name.as_deref())
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let document_id = storage::create_stable_document_id(&knowledge_base.id, &relative_path);
    let created_document_path = PathBuf::from(&knowledge_base.path).join(&relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "create_document",
        &knowledge_base.id,
        "document",
        &document_id,
        &relative_path,
        &created_document_path,
    );
    let new_document = crate::domain::WorkspaceDocument {
        id: document_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: document_title_from_path(&relative_path),
        path: relative_path,
        file_type: "txt".to_owned(),
        updated_at,
        content_hash: storage::hash_content(""),
        content: Some(String::new()),
        preview_available: false,
    };

    snapshot.documents.insert(0, new_document);
    snapshot.knowledge_bases[knowledge_base_index].document_count += 1;
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count += 1;
        *scan_report
            .scanned_by_type
            .entry("txt".to_owned())
            .or_insert(0) += 1;
    }
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    snapshot.active_note_id.clear();
    snapshot.active_document_id = document_id;
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_document",
            "completed",
            "已创建 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", snapshot.active_document_id.clone())
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 用户在目录树的指定目录下新建文件夹，成功后只更新目录快照不切换当前笔记。 */
#[tauri::command]
pub async fn create_folder(
    app: AppHandle,
    payload: CreateFolderPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|knowledge_base| knowledge_base.id == payload.knowledge_base_id)
        .ok_or_else(|| "找不到要新建文件夹的知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();

    if knowledge_base.status == "error" {
        return Err("当前知识库目录不可访问，无法新建文件夹。".to_owned());
    }

    let root_path = PathBuf::from(&knowledge_base.path);
    let parent_path = payload.parent_path;
    let folder_name = payload.folder_name;
    let relative_path = run_blocking("创建文件夹", move || {
        storage::create_folder(&root_path, &parent_path, &folder_name)
    })
    .await?;
    let created_relative_path = relative_path.clone();
    let folder_id = storage::create_stable_folder_id(&knowledge_base.id, &relative_path);
    let folder_entry = FolderEntry {
        id: folder_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        name: folder_name_from_path(&relative_path),
        path: relative_path,
        updated_at: "刚刚".to_owned(),
    };

    // 快照可能来自旧版本或浏览器 fallback，追加前去重，避免同一目录显示两次。
    snapshot.folders.retain(|folder| {
        // 只在当前知识库内去重新建目录，不能影响其他知识库中同名相对目录。
        folder.knowledge_base_id != knowledge_base.id
            || (folder.id != folder_entry.id && folder.path != folder_entry.path)
    });
    snapshot.folders.push(folder_entry);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();
    snapshot.active_knowledge_base_id = knowledge_base.id.clone();
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "create_folder",
            "completed",
            "已创建文件夹。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("folder", folder_id)
        .relative_path(created_relative_path),
    );

    Ok(snapshot)
}

/** 重命名 Markdown 文件，只修改文件名，并同步更新快照与会话引用。 */
#[tauri::command]
pub async fn rename_note(
    app: AppHandle,
    payload: RenameNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要重命名的笔记".to_owned())?;
    let previous_note = snapshot.notes[note_index].clone();
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == previous_note.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);
    let current_relative_path = previous_note.path.clone();
    let next_file_name = payload.next_file_name;
    let (next_relative_path, current_content, current_hash) =
        run_blocking("重命名 Markdown 文件", move || {
            storage::rename_markdown_file(&root_path, &current_relative_path, &next_file_name)
        })
        .await?;
    let next_note_id = storage::create_stable_note_id(&knowledge_base.id, &next_relative_path);
    let next_title =
        storage::extract_markdown_title(Path::new(&next_relative_path), &current_content);
    let history_migrate_app = app.clone();
    let history_previous_note_id = payload.note_id.clone();
    let history_next_note_id = next_note_id.clone();
    let history_knowledge_base_id = knowledge_base.id.clone();
    let history_next_relative_path = next_relative_path.clone();
    let history_next_title = next_title.clone();

    if let Err(_) = run_blocking("迁移 Markdown 历史记录", move || {
        storage::migrate_document_history_target(
            &history_migrate_app,
            "note",
            &history_previous_note_id,
            &history_next_note_id,
            &history_knowledge_base_id,
            &history_next_relative_path,
            &history_next_title,
        )
    })
    .await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "document_history_migration",
                "failed",
                "Markdown 重命名后历史记录迁移失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base.id.clone())
            .entity("note", next_note_id.clone())
            .relative_path(next_relative_path.clone())
            .metadata(json!({
                "targetKind": "note",
                "failureKind": "migration_failed",
            })),
        );
    }
    let next_note_path = PathBuf::from(&knowledge_base.path).join(&next_relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "rename_note",
        &knowledge_base.id,
        "note",
        &next_note_id,
        &next_relative_path,
        &next_note_path,
    );

    snapshot.notes[note_index].id = next_note_id.clone();
    snapshot.notes[note_index].title = next_title;
    snapshot.notes[note_index].path = next_relative_path.clone();
    snapshot.notes[note_index].content = current_content;
    snapshot.notes[note_index].content_hash = current_hash;
    snapshot.notes[note_index].updated_at = updated_at;
    snapshot.active_document_id.clear();

    replace_note_reference_after_rename(
        &mut snapshot,
        &payload.note_id,
        &next_note_id,
        &next_relative_path,
    );
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "rename_note",
            "completed",
            "已重命名 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", next_note_id)
        .relative_path(next_relative_path),
    );

    Ok(snapshot)
}

/** 重命名 txt 文档，只修改文件名，并同步更新快照。 */
#[tauri::command]
pub async fn rename_document(
    app: AppHandle,
    payload: RenameDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要重命名的文档".to_owned())?;
    let previous_document = snapshot.documents[document_index].clone();

    if previous_document.file_type != "txt" {
        return Err("只有 TXT 文档支持重命名。".to_owned());
    }

    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == previous_document.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);
    let current_relative_path = previous_document.path.clone();
    let next_file_name = payload.next_file_name;
    let (next_relative_path, current_content, current_hash) =
        run_blocking("重命名 TXT 文件", move || {
            storage::rename_text_document_file(&root_path, &current_relative_path, &next_file_name)
        })
        .await?;
    let next_document_id =
        storage::create_stable_document_id(&knowledge_base.id, &next_relative_path);
    let next_document_title = document_title_from_path(&next_relative_path);
    let history_migrate_app = app.clone();
    let history_previous_document_id = payload.document_id.clone();
    let history_next_document_id = next_document_id.clone();
    let history_knowledge_base_id = knowledge_base.id.clone();
    let history_next_relative_path = next_relative_path.clone();
    let history_next_title = next_document_title.clone();

    if let Err(_) = run_blocking("迁移 TXT 历史记录", move || {
        storage::migrate_document_history_target(
            &history_migrate_app,
            "document",
            &history_previous_document_id,
            &history_next_document_id,
            &history_knowledge_base_id,
            &history_next_relative_path,
            &history_next_title,
        )
    })
    .await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "document_history_migration",
                "failed",
                "TXT 重命名后历史记录迁移失败。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base.id.clone())
            .entity("document", next_document_id.clone())
            .relative_path(next_relative_path.clone())
            .metadata(json!({
                "targetKind": "document",
                "failureKind": "migration_failed",
            })),
        );
    }
    let next_document_path = PathBuf::from(&knowledge_base.path).join(&next_relative_path);
    let updated_at = read_file_updated_at_or_now(
        &app,
        "rename_document",
        &knowledge_base.id,
        "document",
        &next_document_id,
        &next_relative_path,
        &next_document_path,
    );

    snapshot.documents[document_index].id = next_document_id.clone();
    snapshot.documents[document_index].title = next_document_title;
    snapshot.documents[document_index].path = next_relative_path;
    snapshot.documents[document_index].content = Some(current_content);
    snapshot.documents[document_index].content_hash = current_hash;
    snapshot.documents[document_index].updated_at = updated_at;

    if snapshot.active_document_id == payload.document_id {
        snapshot.active_document_id = next_document_id;
        snapshot.active_note_id.clear();
    }

    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "rename_document",
            "completed",
            "已重命名 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", snapshot.documents[document_index].id.clone())
        .relative_path(snapshot.documents[document_index].path.clone()),
    );

    Ok(snapshot)
}

/** 删除 Markdown 文件到系统回收站，并从快照中移除笔记和相关会话引用。 */
#[tauri::command]
pub async fn delete_note(
    app: AppHandle,
    payload: DeleteNotePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要删除的笔记".to_owned())?;
    let note = snapshot.notes[note_index].clone();
    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|item| item.id == note.knowledge_base_id)
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let relative_path = note.path.clone();
    let expected_hash = payload.expected_hash;

    run_blocking("删除 Markdown 文件", move || {
        storage::trash_markdown_file(&root_path, &relative_path, &expected_hash)
    })
    .await?;

    clear_document_history_after_delete_best_effort(
        &app,
        "note",
        note.id.clone(),
        knowledge_base.id.clone(),
        note.path.clone(),
        started_at,
    )
    .await;

    snapshot.notes.remove(note_index);
    snapshot.knowledge_bases[knowledge_base_index].note_count = snapshot.knowledge_bases
        [knowledge_base_index]
        .note_count
        .saturating_sub(1);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();

    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count = scan_report.scanned_file_count.saturating_sub(1);
    }

    remove_note_references_after_delete(&mut snapshot, &payload.note_id);
    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "delete_note",
            "completed",
            "已将 Markdown 笔记移入回收站。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("note", note.id)
        .relative_path(note.path),
    );

    Ok(snapshot)
}

/** 删除 txt 文档到系统回收站，并从快照中移除普通文档引用。 */
#[tauri::command]
pub async fn delete_document(
    app: AppHandle,
    payload: DeleteDocumentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要删除的文档".to_owned())?;
    let document = snapshot.documents[document_index].clone();

    if document.file_type != "txt" {
        return Err("只有 TXT 文档支持删除。".to_owned());
    }

    let knowledge_base_index = snapshot
        .knowledge_bases
        .iter()
        .position(|item| item.id == document.knowledge_base_id)
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let knowledge_base = snapshot.knowledge_bases[knowledge_base_index].clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let relative_path = document.path.clone();
    let expected_hash = payload.expected_hash;

    run_blocking("删除 TXT 文件", move || {
        storage::trash_text_document_file(&root_path, &relative_path, &expected_hash)
    })
    .await?;

    clear_document_history_after_delete_best_effort(
        &app,
        "document",
        document.id.clone(),
        knowledge_base.id.clone(),
        document.path.clone(),
        started_at,
    )
    .await;

    snapshot.documents.remove(document_index);
    snapshot.knowledge_bases[knowledge_base_index].document_count = snapshot.knowledge_bases
        [knowledge_base_index]
        .document_count
        .saturating_sub(1);
    snapshot.knowledge_bases[knowledge_base_index].updated_at = "刚刚".to_owned();

    if let Some(scan_report) = &mut snapshot.knowledge_bases[knowledge_base_index].scan_report {
        scan_report.scanned_file_count = scan_report.scanned_file_count.saturating_sub(1);
        if let Some(txt_count) = scan_report.scanned_by_type.get_mut("txt") {
            *txt_count = txt_count.saturating_sub(1);
        }
    }

    if snapshot.active_document_id == payload.document_id {
        snapshot.active_document_id.clear();
    }

    normalize_active_entities(&mut snapshot, Some(&knowledge_base.id));
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "delete_document",
            "completed",
            "已将 TXT 文档移入回收站。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base.id)
        .entity("document", document.id)
        .relative_path(document.path),
    );

    Ok(snapshot)
}

/** 保存当前笔记正文，校验知识库边界和文件 hash 后原子写回 Markdown。 */
#[tauri::command]
pub async fn save_note_content(
    app: AppHandle,
    payload: SaveNoteContentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let note_index = snapshot
        .notes
        .iter()
        .position(|note| note.id == payload.note_id)
        .ok_or_else(|| "找不到要保存的笔记".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == snapshot.notes[note_index].knowledge_base_id)
        .ok_or_else(|| "找不到笔记所属知识库".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let note_relative_path = snapshot.notes[note_index].path.clone();
    let target_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &note_relative_path,
    )?;

    let read_path = target_path.clone();
    let current_content = run_blocking("读取待保存 Markdown 文件", move || {
        fs::read_to_string(&read_path)
            .map_err(|error| format!("无法读取待保存 Markdown 文件：{error}"))
    })
    .await?;
    let current_hash = storage::hash_content(&current_content);

    // expectedHash 来自用户开始编辑时的文件版本；不一致说明外部编辑器已改动，必须先重扫。
    if current_hash != payload.expected_hash {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "save_note_content",
                "blocked",
                "目标 Markdown 文件已被外部修改，已阻止保存。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id.clone())
            .entity("note", payload.note_id.clone())
            .relative_path(note_relative_path.clone()),
        );

        return Err("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。".to_owned());
    }

    capture_document_history_before_write(
        &app,
        storage::DocumentHistoryCapture {
            target_kind: "note".to_owned(),
            knowledge_base_id: knowledge_base_id.clone(),
            target_id: payload.note_id.clone(),
            relative_path: note_relative_path.clone(),
            title: snapshot.notes[note_index].title.clone(),
            file_type: "markdown".to_owned(),
            content: current_content,
            source: "manual-save".to_owned(),
            session_id: None,
            change_id: None,
            operation_id: None,
        },
        AppLogCategory::Editor,
        "save_note_content",
        started_at,
    )
    .await?;

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 Markdown 文件", move || {
        storage::atomic_write_markdown(&write_path, &write_content)
    })
    .await?;

    let updated_at = read_file_updated_at_or_now(
        &app,
        "save_note_content",
        &knowledge_base_id,
        "note",
        &payload.note_id,
        &note_relative_path,
        &target_path,
    );
    let next_hash = storage::hash_content(&payload.content);
    snapshot.notes[note_index].content = payload.content;
    snapshot.notes[note_index].content_hash = next_hash;
    snapshot.notes[note_index].updated_at = updated_at;
    snapshot.active_note_id = payload.note_id;
    snapshot.active_document_id.clear();
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "save_note_content",
            "completed",
            "已保存 Markdown 笔记。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .entity("note", snapshot.active_note_id.clone())
        .relative_path(note_relative_path),
    );

    Ok(snapshot)
}

/** 保存当前笔记粘贴的图片附件，只负责落盘和返回 Markdown 片段，不写回正文。 */
#[tauri::command]
pub async fn save_note_image_attachments(
    app: AppHandle,
    payload: SaveNoteImageAttachmentsPayload,
) -> Result<Vec<crate::domain::SavedNoteImageAttachment>, String> {
    let started_at = Instant::now();
    let note_count = payload.images.len();
    let note_id = payload.note_id.clone();
    let snapshot = payload.snapshot;
    let note = snapshot
        .notes
        .iter()
        .find(|item| item.id == note_id)
        .ok_or_else(|| "找不到要保存图片的 Markdown 笔记。".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == note.knowledge_base_id)
        .ok_or_else(|| "找不到图片附件所属知识库。".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let note_entity_id = note.id.clone();
    let root_path = PathBuf::from(&knowledge_base.path);
    let note_relative_path = note.path.clone();
    let write_note_relative_path = note_relative_path.clone();
    let images = payload.images;

    let save_result = run_blocking("保存粘贴图片附件", move || {
        storage::save_note_image_attachments(&root_path, &write_note_relative_path, &images)
    })
    .await;

    match save_result {
        Ok(saved_attachments) => {
            let total_byte_size: usize = saved_attachments
                .iter()
                .map(|attachment| attachment.byte_size)
                .sum();

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Info,
                    AppLogCategory::Editor,
                    "paste_image_attachment",
                    "completed",
                    "已保存粘贴图片附件。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(knowledge_base_id.clone())
                .entity("note", note_entity_id.clone())
                .relative_path(note_relative_path.clone())
                .metadata(json!({
                    "imageCount": note_count,
                    "savedCount": saved_attachments.len(),
                    "totalBytes": total_byte_size,
                })),
            );

            Ok(saved_attachments)
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "paste_image_attachment",
                    "failed",
                    "粘贴图片附件保存失败。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(knowledge_base_id)
                .entity("note", note_entity_id)
                .relative_path(note_relative_path)
                .metadata(json!({
                    "imageCount": note_count,
                })),
            );

            Err(error)
        }
    }
}

/** 保存当前 txt 文档正文，校验知识库边界和文件 hash 后原子写回本地文件。 */
#[tauri::command]
pub async fn save_document_content(
    app: AppHandle,
    payload: SaveDocumentContentPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let document_index = snapshot
        .documents
        .iter()
        .position(|document| document.id == payload.document_id)
        .ok_or_else(|| "找不到要保存的文档".to_owned())?;

    if snapshot.documents[document_index].file_type != "txt" {
        return Err("只有 TXT 文档支持保存。".to_owned());
    }

    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == snapshot.documents[document_index].knowledge_base_id)
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let knowledge_base_id = knowledge_base.id.clone();
    let document_relative_path = snapshot.documents[document_index].path.clone();
    let target_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&knowledge_base.path).as_path(),
        &document_relative_path,
    )?;

    let read_path = target_path.clone();
    let current_content = run_blocking("读取待保存 TXT 文件", move || {
        fs::read_to_string(&read_path).map_err(|error| format!("无法读取待保存 TXT 文件：{error}"))
    })
    .await?;
    let current_hash = storage::hash_content(&current_content);

    // expectedHash 来自用户开始编辑时的文件版本；不一致说明外部编辑器已改动，必须先重扫。
    if current_hash != payload.expected_hash {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Editor,
                "save_document_content",
                "blocked",
                "目标 TXT 文件已被外部修改，已阻止保存。",
            )
            .duration(started_at.elapsed())
            .knowledge_base_id(knowledge_base_id.clone())
            .entity("document", payload.document_id.clone())
            .relative_path(document_relative_path.clone()),
        );

        return Err("目标文件已被外部修改，已阻止保存。请重新扫描后再编辑。".to_owned());
    }

    capture_document_history_before_write(
        &app,
        storage::DocumentHistoryCapture {
            target_kind: "document".to_owned(),
            knowledge_base_id: knowledge_base_id.clone(),
            target_id: payload.document_id.clone(),
            relative_path: document_relative_path.clone(),
            title: snapshot.documents[document_index].title.clone(),
            file_type: "txt".to_owned(),
            content: current_content,
            source: "manual-save".to_owned(),
            session_id: None,
            change_id: None,
            operation_id: None,
        },
        AppLogCategory::Editor,
        "save_document_content",
        started_at,
    )
    .await?;

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 TXT 文件", move || {
        storage::atomic_write_text_document(&write_path, &write_content)
    })
    .await?;

    let updated_at = read_file_updated_at_or_now(
        &app,
        "save_document_content",
        &knowledge_base_id,
        "document",
        &payload.document_id,
        &document_relative_path,
        &target_path,
    );
    let next_hash = storage::hash_content(&payload.content);
    snapshot.documents[document_index].content = Some(payload.content);
    snapshot.documents[document_index].content_hash = next_hash;
    snapshot.documents[document_index].updated_at = updated_at;
    snapshot.active_note_id.clear();
    snapshot.active_document_id = payload.document_id;
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "save_document_content",
            "completed",
            "已保存 TXT 文档。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(knowledge_base_id)
        .entity("document", snapshot.active_document_id.clone())
        .relative_path(document_relative_path),
    );

    Ok(snapshot)
}

/** 读取当前 Markdown/TXT 文件的历史记录列表；正文详情按需单独加载。 */
#[tauri::command]
pub async fn load_document_history(
    app: AppHandle,
    payload: LoadDocumentHistoryPayload,
) -> Result<Vec<DocumentHistoryEntry>, String> {
    let started_at = Instant::now();
    let target = resolve_document_history_target(
        &payload.snapshot,
        &payload.target_kind,
        &payload.target_id,
    )?;
    let target_kind = target.target_kind.clone();
    let target_id = target.entity_id.clone();
    let history_app = app.clone();
    let entries = run_blocking("读取文档历史记录", move || {
        storage::load_document_history(&history_app, &target_kind, &target_id)
    })
    .await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "load_document_history",
            "completed",
            "已读取文档历史记录。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(target.knowledge_base_id)
        .entity(target.entity_type, target.entity_id)
        .relative_path(target.relative_path)
        .metadata(json!({
            "targetKind": target.target_kind,
            "fileType": target.file_type,
            "entryCount": entries.len(),
        })),
    );

    Ok(entries)
}

/** 读取单条历史记录正文快照，供前端 diff 和恢复确认使用。 */
#[tauri::command]
pub async fn load_document_history_entry(
    app: AppHandle,
    payload: LoadDocumentHistoryEntryPayload,
) -> Result<DocumentHistoryEntryDetail, String> {
    run_blocking("读取文档历史详情", move || {
        storage::load_document_history_entry(&app, &payload.entry_id)
    })
    .await
}

/** 清空当前文件历史记录；只删除历史快照，不删除用户文档。 */
#[tauri::command]
pub async fn clear_document_history(
    app: AppHandle,
    payload: ClearDocumentHistoryPayload,
) -> Result<(), String> {
    let started_at = Instant::now();
    let target = resolve_document_history_target(
        &payload.snapshot,
        &payload.target_kind,
        &payload.target_id,
    )?;
    let target_kind = target.target_kind.clone();
    let target_id = target.entity_id.clone();
    let clear_app = app.clone();
    let clear_result = run_blocking("清空文档历史记录", move || {
        storage::clear_document_history(&clear_app, &target_kind, &target_id)
    })
    .await;

    match clear_result {
        Ok(summary) => {
            let has_cleanup_failures = summary.cleanup_failure_count > 0;

            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    if has_cleanup_failures {
                        AppLogLevel::Warn
                    } else {
                        AppLogLevel::Info
                    },
                    AppLogCategory::Editor,
                    "clear_document_history",
                    if has_cleanup_failures {
                        "partial"
                    } else {
                        "completed"
                    },
                    if has_cleanup_failures {
                        "文档历史记录已清空，但部分快照清理失败。"
                    } else {
                        "已清空文档历史记录。"
                    },
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id)
                .entity(target.entity_type, target.entity_id)
                .relative_path(target.relative_path)
                .metadata(json!({
                    "targetKind": target.target_kind,
                    "fileType": target.file_type,
                    "removedCount": summary.removed_count,
                    "cleanupFailureCount": summary.cleanup_failure_count,
                })),
            );

            Ok(())
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Editor,
                    "clear_document_history",
                    "failed",
                    "清空文档历史记录失败。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id)
                .entity(target.entity_type, target.entity_id)
                .relative_path(target.relative_path)
                .metadata(json!({
                    "targetKind": target.target_kind,
                    "fileType": target.file_type,
                })),
            );

            Err(error)
        }
    }
}

/** 恢复指定历史版本；恢复前先捕获当前版本，所以回档操作本身可撤销。 */
#[tauri::command]
pub async fn restore_document_history_entry(
    app: AppHandle,
    payload: RestoreDocumentHistoryEntryPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let expected_hash = payload.expected_hash;
    let entry_id = payload.entry_id;
    let mut snapshot = payload.snapshot;
    let load_app = app.clone();
    let entry_id_for_load = entry_id.clone();
    let detail = match run_blocking("读取待恢复历史版本", move || {
        storage::load_document_history_entry(&load_app, &entry_id_for_load)
    })
    .await
    {
        Ok(detail) => detail,
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "failed",
                    "读取待恢复历史版本失败。",
                )
                .duration(started_at.elapsed())
                .entity("history", entry_id),
            );

            return Err(error);
        }
    };
    let target = resolve_document_history_target(
        &snapshot,
        &detail.entry.target_kind,
        &detail.entry.target_id,
    )?;

    if detail.entry.file_type != target.file_type {
        return Err("历史版本文件类型与当前文件不一致，已阻止恢复。".to_owned());
    }

    if target.target_kind == "note" {
        let note_index = snapshot
            .notes
            .iter()
            .position(|note| note.id == target.entity_id)
            .ok_or_else(|| "找不到要恢复的 Markdown 笔记。".to_owned())?;
        let knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == target.knowledge_base_id)
            .cloned()
            .ok_or_else(|| "找不到笔记所属知识库。".to_owned())?;
        let target_path = storage::resolve_existing_file_inside_root(
            PathBuf::from(&knowledge_base.path).as_path(),
            &target.relative_path,
        )?;
        let read_path = target_path.clone();
        let current_content = run_blocking("读取待恢复 Markdown 文件", move || {
            fs::read_to_string(&read_path)
                .map_err(|error| format!("无法读取待恢复 Markdown 文件：{error}"))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        if current_hash != expected_hash {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "blocked",
                    "目标 Markdown 文件已被外部修改，已阻止恢复。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id.clone())
                .entity(target.entity_type, target.entity_id.clone())
                .relative_path(target.relative_path.clone())
                .metadata(json!({ "entryId": detail.entry.id.clone() })),
            );

            return Err("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。".to_owned());
        }

        capture_document_history_before_write(
            &app,
            storage::DocumentHistoryCapture {
                target_kind: target.target_kind.clone(),
                knowledge_base_id: target.knowledge_base_id.clone(),
                target_id: target.entity_id.clone(),
                relative_path: target.relative_path.clone(),
                title: target.title.clone(),
                file_type: target.file_type.clone(),
                content: current_content,
                source: "restore".to_owned(),
                session_id: None,
                change_id: None,
                operation_id: None,
            },
            AppLogCategory::Editor,
            "restore_document_history_entry",
            started_at,
        )
        .await?;

        let write_path = target_path.clone();
        let restored_content = detail.content.clone();

        run_blocking("恢复 Markdown 历史版本", move || {
            storage::atomic_write_markdown(&write_path, &restored_content)
        })
        .await?;

        let updated_at = read_file_updated_at_or_now(
            &app,
            "restore_document_history_entry",
            &target.knowledge_base_id,
            target.entity_type,
            &target.entity_id,
            &target.relative_path,
            &target_path,
        );
        let next_hash = storage::hash_content(&detail.content);

        snapshot.notes[note_index].content = detail.content.clone();
        snapshot.notes[note_index].content_hash = next_hash;
        snapshot.notes[note_index].updated_at = updated_at;
        snapshot.active_note_id = target.entity_id.clone();
        snapshot.active_document_id.clear();
    } else {
        let document_index = snapshot
            .documents
            .iter()
            .position(|document| document.id == target.entity_id)
            .ok_or_else(|| "找不到要恢复的 TXT 文档。".to_owned())?;
        let knowledge_base = snapshot
            .knowledge_bases
            .iter()
            .find(|item| item.id == target.knowledge_base_id)
            .cloned()
            .ok_or_else(|| "找不到文档所属知识库。".to_owned())?;
        let target_path = storage::resolve_existing_file_inside_root(
            PathBuf::from(&knowledge_base.path).as_path(),
            &target.relative_path,
        )?;
        let read_path = target_path.clone();
        let current_content = run_blocking("读取待恢复 TXT 文件", move || {
            fs::read_to_string(&read_path)
                .map_err(|error| format!("无法读取待恢复 TXT 文件：{error}"))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        if current_hash != expected_hash {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Editor,
                    "restore_document_history_entry",
                    "blocked",
                    "目标 TXT 文件已被外部修改，已阻止恢复。",
                )
                .duration(started_at.elapsed())
                .knowledge_base_id(target.knowledge_base_id.clone())
                .entity(target.entity_type, target.entity_id.clone())
                .relative_path(target.relative_path.clone())
                .metadata(json!({ "entryId": detail.entry.id.clone() })),
            );

            return Err("目标文件已被外部修改，已阻止恢复。请重新扫描后再操作。".to_owned());
        }

        capture_document_history_before_write(
            &app,
            storage::DocumentHistoryCapture {
                target_kind: target.target_kind.clone(),
                knowledge_base_id: target.knowledge_base_id.clone(),
                target_id: target.entity_id.clone(),
                relative_path: target.relative_path.clone(),
                title: target.title.clone(),
                file_type: target.file_type.clone(),
                content: current_content,
                source: "restore".to_owned(),
                session_id: None,
                change_id: None,
                operation_id: None,
            },
            AppLogCategory::Editor,
            "restore_document_history_entry",
            started_at,
        )
        .await?;

        let write_path = target_path.clone();
        let restored_content = detail.content.clone();

        run_blocking("恢复 TXT 历史版本", move || {
            storage::atomic_write_text_document(&write_path, &restored_content)
        })
        .await?;

        let updated_at = read_file_updated_at_or_now(
            &app,
            "restore_document_history_entry",
            &target.knowledge_base_id,
            target.entity_type,
            &target.entity_id,
            &target.relative_path,
            &target_path,
        );
        let next_hash = storage::hash_content(&detail.content);

        snapshot.documents[document_index].content = Some(detail.content.clone());
        snapshot.documents[document_index].content_hash = next_hash;
        snapshot.documents[document_index].updated_at = updated_at;
        snapshot.active_note_id.clear();
        snapshot.active_document_id = target.entity_id.clone();
    }

    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "restore_document_history_entry",
            "completed",
            "已恢复文档历史版本。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(target.knowledge_base_id)
        .entity(target.entity_type, target.entity_id)
        .relative_path(target.relative_path)
        .metadata(json!({
            "entryId": detail.entry.id.clone(),
            "targetKind": target.target_kind,
            "fileType": target.file_type,
            "byteSize": detail.entry.byte_size,
            "lineCount": detail.entry.line_count,
        })),
    );

    Ok(snapshot)
}

/** 加载 docx/pdf/图片文档预览，命令层负责定位知识库并把路径授权给 asset protocol。 */
#[tauri::command]
pub async fn load_document_preview(
    app: AppHandle,
    payload: LoadDocumentPreviewPayload,
) -> Result<DocumentPreview, String> {
    let snapshot = payload.snapshot;
    let document = snapshot
        .documents
        .iter()
        .find(|document| document.id == payload.document_id)
        .cloned()
        .ok_or_else(|| "找不到要预览的文档".to_owned())?;
    let knowledge_base = snapshot
        .knowledge_bases
        .iter()
        .find(|item| item.id == document.knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到文档所属知识库".to_owned())?;
    let root_path = PathBuf::from(&knowledge_base.path);

    allow_asset_protocol_directory(&app, &root_path)?;

    run_blocking("加载文档预览", move || {
        storage::load_document_preview(&root_path, &document)
    })
    .await
}

/** 移除知识库授权记录和本地索引缓存，不删除用户目录中的 Markdown 文件。 */
#[tauri::command]
pub async fn remove_knowledge_base(
    app: AppHandle,
    payload: RemoveKnowledgeBasePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let removed_knowledge_base_id = payload.knowledge_base_id.clone();
    let mut snapshot = payload.snapshot;

    snapshot
        .knowledge_bases
        .retain(|knowledge_base| knowledge_base.id != payload.knowledge_base_id);
    snapshot
        .notes
        .retain(|note| note.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .folders
        .retain(|folder| folder.knowledge_base_id != payload.knowledge_base_id);
    snapshot
        .documents
        .retain(|document| document.knowledge_base_id != payload.knowledge_base_id);

    // 会话只移除目标知识库范围；失去全部范围的会话同步删除，避免保留不可用上下文。
    snapshot.sessions.retain_mut(|session| {
        session
            .knowledge_base_ids
            .retain(|id| id != &payload.knowledge_base_id);
        session
            .pinned_note_ids
            .retain(|note_id| snapshot.notes.iter().any(|note| note.id == *note_id));

        if session
            .active_note_id
            .as_ref()
            .is_some_and(|note_id| !snapshot.notes.iter().any(|note| note.id == *note_id))
        {
            session.active_note_id = None;
        }

        !session.knowledge_base_ids.is_empty()
    });

    normalize_knowledge_base_flags(&mut snapshot);
    normalize_active_entities(&mut snapshot, None);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::KnowledgeBase,
            "remove_knowledge_base",
            "completed",
            "已移除知识库授权。",
        )
        .duration(started_at.elapsed())
        .knowledge_base_id(removed_knowledge_base_id),
    );

    Ok(snapshot)
}

/** 运行 Agent 单轮 loop，检索作为工具由 Agent 自行选择。 */
#[tauri::command]
pub async fn run_agent_turn(
    app: AppHandle,
    payload: AgentTurnPayload,
) -> Result<AgentTurnResult, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let mut snapshot = hydrate_persisted_sessions_for_turn(&app, payload.snapshot).await?;
    let request = payload.request;
    let session_id = request.session_id.clone();
    let active_knowledge_base_id = request.active_knowledge_base_id.clone();
    let request_metadata = json!({ "action": request.action.clone() });

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "run_agent_turn",
            "started",
            "Agent 开始处理用户请求。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone())
        .knowledge_base_id(active_knowledge_base_id.clone())
        .metadata(request_metadata),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let skills_app = app.clone();
    let available_skills = run_blocking("读取 Agent Skills", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::load_agent_skills(&skills_app, &connection)
    })
    .await?;

    // request 中的 active 信息来自 UI 当前焦点；会话 scope 已由 SQLite 中恢复的 session 决定。
    snapshot.active_knowledge_base_id = request.active_knowledge_base_id.clone();
    snapshot.active_note_id = request.active_note_id.clone();
    if snapshot
        .sessions
        .iter()
        .any(|session| session.id == request.session_id)
    {
        snapshot.active_session_id = request.session_id.clone();
    }

    let runtime_result =
        runtime::run_agent_turn(&app, snapshot, request, settings, available_skills).await;
    let audit_app = app.clone();
    let audit_log = runtime_result.audit_log.clone();

    if let Err(error) = run_blocking("写入请求审计日志", move || {
        storage::append_request_audit_log(&audit_app, &audit_log)
    })
    .await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "run_agent_turn",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .knowledge_base_id(active_knowledge_base_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }

    // 每轮后刷新本地索引并持久化会话，确保消息、工具轨迹和 pending diff 可在重启后恢复。
    if let Err(error) =
        index_snapshot_in_background(app.clone(), &runtime_result.turn_result.snapshot).await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "run_agent_turn",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .knowledge_base_id(active_knowledge_base_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "run_agent_turn",
            "completed",
            "Agent 已完成本轮处理。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .knowledge_base_id(active_knowledge_base_id)
        .duration(started_at.elapsed())
        .metadata(json!({
            "auditKind": runtime_result.audit_log.kind.clone(),
            "toolSummary": runtime_result.audit_log.tool_summary.clone(),
        })),
    );

    Ok(runtime_result.turn_result)
}

/** 手动整理当前 Agent 会话工作记忆，成功后持久化会话快照。 */
#[tauri::command]
pub async fn compact_agent_context(
    app: AppHandle,
    payload: CompactAgentContextPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let session_id = payload.session_id.clone();

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "compact_agent_context",
            "started",
            "开始整理 Agent 会话上下文。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone()),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let snapshot = hydrate_persisted_sessions_for_turn(&app, payload.snapshot).await?;
    let snapshot = runtime::compact_agent_context_summary(snapshot, &session_id, settings).await?;

    if let Err(error) = index_snapshot_in_background(app.clone(), &snapshot).await {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "compact_agent_context",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "compact_agent_context",
            "completed",
            "已整理 Agent 会话上下文。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .duration(started_at.elapsed()),
    );

    Ok(snapshot)
}

/** IM 后台入口复用 Agent runtime：创建或复用映射会话，持久化消息、审计和索引。 */
pub(crate) async fn run_agent_turn_from_im(
    app: AppHandle,
    provider_id: String,
    prompt: String,
    channel_key: String,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 工作台状态", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let valid_scope_ids = snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| knowledge_base_ids.iter().any(|id| id == &knowledge_base.id))
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect::<Vec<_>>();

    if valid_scope_ids.is_empty() {
        return Err("IM 默认知识库范围为空或已失效。".to_owned());
    }

    let session_resolution = resolve_or_create_im_session(
        &app,
        &mut snapshot,
        &channel_key,
        &im_identity,
        valid_scope_ids.clone(),
    )?;
    let session_id = session_resolution.session_id;
    if let Some(pending_change) = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.pending_change.as_ref())
        .filter(|change| change.status == "pending")
    {
        // 一个 IM 会话只允许存在一个待确认变更，避免下一轮 Agent 覆盖远程用户尚未处理的 diff。
        return Err(format!(
            "当前有待确认变更 {}；请先发送“详情 {}”、“确认 {}”或“取消 {}”。",
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
        ));
    }
    let active_knowledge_base_id = valid_scope_ids.first().cloned().unwrap_or_default();
    let user_message = crate::im::build_im_user_message(&prompt);
    let user_message_id = user_message.id.clone();

    if let Some(session) = snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        session.messages.push(user_message);
        session.updated_at = storage::format_local_datetime();
    }

    snapshot.active_session_id = session_id.clone();
    snapshot.active_knowledge_base_id = active_knowledge_base_id.clone();
    snapshot.active_note_id.clear();
    snapshot.active_document_id.clear();

    storage::save_sessions(&app, &snapshot)?;

    logging::write_app_event_best_effort(
        &app,
        build_im_identity_event(
            &session_id,
            &im_identity,
            session_resolution.identity_status,
        ),
    );

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_agent_turn",
            "started",
            "IM 消息开始进入 Agent runtime。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone())
        .knowledge_base_id(active_knowledge_base_id.clone())
        .metadata(json!({
            "providerId": provider_id.clone(),
            "scopeCount": valid_scope_ids.len(),
            "promptChars": prompt.chars().count(),
            "channelHash": storage::hash_content(&channel_key).chars().take(16).collect::<String>(),
        })),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let skills_app = app.clone();
    let available_skills = run_blocking("读取 Agent Skills", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::load_agent_skills(&skills_app, &connection)
    })
    .await?;
    let request = crate::im::build_im_turn_request(
        prompt,
        session_id.clone(),
        active_knowledge_base_id.clone(),
        user_message_id,
    );
    let runtime_result =
        runtime::run_agent_turn(&app, snapshot, request, settings, available_skills).await;
    let audit_app = app.clone();
    let audit_log = runtime_result.audit_log.clone();

    run_blocking("写入 IM 请求审计日志", move || {
        storage::append_request_audit_log(&audit_app, &audit_log)
    })
    .await?;
    index_snapshot_in_background(app.clone(), &runtime_result.turn_result.snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_agent_turn",
            "completed",
            "IM 消息已完成 Agent runtime 处理。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .knowledge_base_id(active_knowledge_base_id)
        .duration(started_at.elapsed())
        .metadata(json!({
            "providerId": provider_id,
            "auditKind": runtime_result.audit_log.kind,
            "toolSummary": runtime_result.audit_log.tool_summary,
        })),
    );

    Ok(runtime_result.turn_result.snapshot)
}

/**
 * 处理 provider 无关的 IM 内置指令。调用方必须先完成鉴权、去重与群聊 @ 门禁；
 * 本入口仅从持久化状态读取会话，避免远端事件携带过期的工作台快照覆盖本地数据。
 */
pub(crate) async fn handle_im_builtin_command(
    app: AppHandle,
    provider_id: &str,
    command: crate::im::ImBuiltinCommand,
    channel_key: &str,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> String {
    let started_at = Instant::now();
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let command_name = match command {
        crate::im::ImBuiltinCommand::Help => "help",
        crate::im::ImBuiltinCommand::New => "new",
        crate::im::ImBuiltinCommand::Compact => "compact",
    };

    let result = match command {
        crate::im::ImBuiltinCommand::Help => Ok(ImBuiltinCommandResult {
            reply: crate::im::builtin_command_help_text().to_owned(),
            session_id: None,
            message_count: None,
            summary_chars: None,
        }),
        crate::im::ImBuiltinCommand::New => {
            create_im_session_from_command(
                &app,
                provider_id,
                channel_key,
                knowledge_base_ids,
                im_identity,
            )
            .await
        }
        crate::im::ImBuiltinCommand::Compact => {
            compact_im_session_from_command(&app, provider_id, channel_key).await
        }
    };

    let (status, reply, session_id, message_count, summary_chars) = match result {
        Ok(result) => (
            "completed",
            result.reply,
            result.session_id,
            result.message_count,
            result.summary_chars,
        ),
        Err(error) => (
            "failed",
            format!("操作失败：{}", logging::sanitize_log_text(&error)),
            None,
            None,
            None,
        ),
    };
    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if status == "failed" {
                AppLogLevel::Warn
            } else {
                AppLogLevel::Info
            },
            AppLogCategory::Im,
            "im_builtin_command",
            status,
            "IM 内置指令已处理。",
        )
        .session_id(session_id.unwrap_or_default())
        .duration(started_at.elapsed())
        .metadata(json!({
            "command": command_name,
            "providerId": provider_id,
            "channelHash": channel_hash,
            "messageCount": message_count,
            "summaryChars": summary_chars,
        })),
    );
    reply
}

/** 内置指令的用户可见结果和可观测性指标；不包含消息正文或摘要内容。 */
struct ImBuiltinCommandResult {
    reply: String,
    session_id: Option<String>,
    message_count: Option<usize>,
    summary_chars: Option<usize>,
}

/** 创建并立即映射空 IM 会话；有待确认文件变更时拒绝切换，保留远程审批入口。 */
async fn create_im_session_from_command(
    app: &AppHandle,
    _provider_id: &str,
    channel_key: &str,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> Result<ImBuiltinCommandResult, String> {
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 新会话状态", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let valid_scope_ids = snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| knowledge_base_ids.iter().any(|id| id == &knowledge_base.id))
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect::<Vec<_>>();
    if valid_scope_ids.is_empty() {
        return Err("IM 默认知识库范围为空或已失效。".to_owned());
    }

    if let Some(mapped_id) = storage::load_im_session_mapping(app, channel_key)? {
        if snapshot.sessions.iter().any(|session| {
            session.id == mapped_id
                && session
                    .pending_change
                    .as_ref()
                    .is_some_and(|change| change.status == "pending")
        }) {
            return Ok(ImBuiltinCommandResult {
                reply:
                    "当前会话有待确认变更；请先发送“详情 <编号>”、“确认 <编号>”或“取消 <编号>”。"
                        .to_owned(),
                session_id: Some(mapped_id),
                message_count: None,
                summary_chars: None,
            });
        }
    }

    let new_identity = crate::im::build_im_new_session_identity(&im_identity);
    // 明确使用固定摘要，保证命令本身不会成为新会话的标题或可见主题。
    let session = crate::im::build_im_agent_session(new_identity, valid_scope_ids);
    let session_id = session.id.clone();
    snapshot.sessions.insert(0, session);
    storage::save_sessions(app, &snapshot)?;
    storage::save_im_session_mapping(app, channel_key, &session_id)?;

    Ok(ImBuiltinCommandResult {
        reply: "已开启新会话，下一条消息将从新的上下文开始。".to_owned(),
        session_id: Some(session_id),
        message_count: Some(0),
        summary_chars: Some(0),
    })
}

/** 压缩当前 channel 的持久化会话；空会话直接提示，绝不为此调用模型。 */
async fn compact_im_session_from_command(
    app: &AppHandle,
    provider_id: &str,
    channel_key: &str,
) -> Result<ImBuiltinCommandResult, String> {
    let Some(session_id) = storage::load_im_session_mapping(app, channel_key)? else {
        return Ok(ImBuiltinCommandResult {
            reply: "当前还没有可整理的会话；请先发送一条普通消息。".to_owned(),
            session_id: None,
            message_count: Some(0),
            summary_chars: Some(0),
        });
    };
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 上下文", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let Some(session_index) = snapshot.sessions.iter().position(|session| {
        session.id == session_id
            && session.im_identity.as_ref().is_some_and(|identity| {
                identity.provider_id == provider_id && identity.channel_hash == channel_hash
            })
    }) else {
        return Ok(ImBuiltinCommandResult {
            reply: "当前 IM 会话不可用，请发送 /new 开启新会话。".to_owned(),
            session_id: Some(session_id),
            message_count: None,
            summary_chars: None,
        });
    };
    let message_count = snapshot.sessions[session_index].messages.len();
    if message_count == 0 {
        return Ok(ImBuiltinCommandResult {
            reply: "当前会话没有可整理的对话消息。".to_owned(),
            session_id: Some(session_id),
            message_count: Some(0),
            summary_chars: Some(0),
        });
    }

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    // 模型调用不可用时压缩为确定性工作记忆，确保移动端命令始终能完成并被持久化。
    snapshot = match runtime::compact_agent_context_summary(snapshot.clone(), &session_id, settings)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!(target: "im", "IM 上下文压缩降级为确定性摘要：session={} reason={}", session_id, logging::sanitize_log_text(&error));
            runtime::update_agent_context_summary_deterministic(
                &mut snapshot,
                session_index,
                Some(&error),
                true,
            );
            snapshot
        }
    };
    let summary = snapshot.sessions[session_index].context_summary.as_ref();
    let summary_chars = summary
        .map(|item| {
            serde_json::to_string(item)
                .unwrap_or_default()
                .chars()
                .count()
        })
        .unwrap_or(0);
    let goal = summary
        .and_then(|item| item.current_goal.as_deref())
        .unwrap_or("未识别当前目标");
    // 压缩摘要可能复述用户输入；回传到群聊前沿用日志脱敏规则，避免意外暴露密钥或本地绝对路径。
    let short_goal = logging::sanitize_log_text(goal)
        .chars()
        .take(48)
        .collect::<String>();
    storage::save_sessions(app, &snapshot)?;
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    Ok(ImBuiltinCommandResult {
        reply: format!(
            "已整理当前会话上下文（{} 条消息）。当前目标：{}",
            message_count, short_goal
        ),
        session_id: Some(session_id),
        message_count: Some(message_count),
        summary_chars: Some(summary_chars),
    })
}

/**
 * 在 IM 会话内处理待确认变更；此入口只信任持久化的会话和 channel 映射，
 * 不接受桌面前端传入的 WorkspaceSnapshot，避免远程确认覆盖本地最新状态。
 */
pub(crate) async fn handle_im_pending_change_command(
    app: AppHandle,
    provider_id: &str,
    channel_key: &str,
    action: &str,
    change_code: &str,
) -> String {
    let started_at = Instant::now();
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let normalized_action = action.trim().to_ascii_lowercase();
    let normalized_code = change_code.trim();

    // 短编号至少六位，既方便手机端输入，也避免将空字符串或过短前缀匹配到其他变更。
    if normalized_code.chars().count() < 6 {
        return "变更编号至少需要 6 位，例如：确认 change-123456。".to_owned();
    }
    if !matches!(normalized_action.as_str(), "confirm" | "cancel" | "details") {
        return "不支持的变更操作。请使用：确认 <编号>、取消 <编号> 或详情 <编号>。".to_owned();
    }

    let snapshot_app = app.clone();
    let mut snapshot = match run_blocking("加载 IM 待确认变更", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return format!("读取待确认变更失败：{}", logging::sanitize_log_text(&error)),
    };
    let session_id = match storage::load_im_session_mapping(&app, channel_key) {
        Ok(Some(session_id)) => session_id,
        Ok(None) => return "当前 IM 会话没有待确认变更。".to_owned(),
        Err(error) => return format!("读取 IM 会话失败：{}", logging::sanitize_log_text(&error)),
    };
    let Some(session_index) = snapshot.sessions.iter().position(|session| {
        session.id == session_id
            // 群聊 channel key 已包含发送人 hash；再次匹配持久化身份，确保其他成员不能确认该变更。
            && session.im_identity.as_ref().is_some_and(|identity| {
                identity.provider_id == provider_id && identity.channel_hash == channel_hash
            })
    }) else {
        log_im_change_approval(
            &app,
            "blocked",
            &normalized_action,
            None,
            &channel_hash,
            started_at,
        );
        return "当前身份无权处理该待确认变更。".to_owned();
    };
    let Some(change) = snapshot.sessions[session_index].pending_change.clone() else {
        return "当前 IM 会话没有待确认变更。".to_owned();
    };
    if change.status != "pending" {
        return format!(
            "变更 {} 已处理（状态：{}），无需重复操作。",
            short_change_code(&change.id),
            change.status
        );
    }
    if !change.id.eq_ignore_ascii_case(normalized_code)
        && !change
            .id
            .to_ascii_lowercase()
            .starts_with(&normalized_code.to_ascii_lowercase())
    {
        return "找不到该编号对应的待确认变更；请检查编号后重试。".to_owned();
    }

    if normalized_action == "details" {
        log_im_change_approval(
            &app,
            "completed",
            "details",
            Some(&change.id),
            &channel_hash,
            started_at,
        );
        return build_im_change_details(&change);
    }

    snapshot.active_session_id = session_id.clone();
    let mut snapshot_before_apply = snapshot.clone();
    let result = if normalized_action == "cancel" {
        reject_proposed_change(app.clone(), ChangePayload { snapshot }).await
    } else {
        apply_proposed_change(app.clone(), ChangePayload { snapshot }).await
    };

    match result {
        Ok(updated_snapshot) => {
            // IM 操作没有前端接收 snapshot，因此必须立即持久化会话状态，重复消息才不会二次写入。
            if let Err(error) = storage::save_sessions(&app, &updated_snapshot) {
                log_im_change_approval(
                    &app,
                    "failed",
                    &normalized_action,
                    Some(&change.id),
                    &channel_hash,
                    started_at,
                );
                return format!(
                    "变更已处理，但保存会话状态失败：{}",
                    logging::sanitize_log_text(&error)
                );
            }
            log_im_change_approval(
                &app,
                "completed",
                &normalized_action,
                Some(&change.id),
                &channel_hash,
                started_at,
            );
            if normalized_action == "cancel" {
                format!(
                    "已取消变更 {}，本地文件未修改。",
                    short_change_code(&change.id)
                )
            } else {
                format!("已确认并写入变更 {}。", short_change_code(&change.id))
            }
        }
        Err(error) => {
            // 冲突不可重试且不能保留为可确认状态；写入失败则保留 pending，允许用户稍后处理。
            if error.contains("已变化") || error.contains("未命中") || error.contains("出现多次")
            {
                if let Some(session) = snapshot_before_apply.sessions.get_mut(session_index) {
                    if let Some(pending_change) = session.pending_change.as_mut() {
                        pending_change.status = "expired".to_owned();
                    }
                    session.updated_at = storage::format_local_datetime();
                }
                let _ = storage::save_sessions(&app, &snapshot_before_apply);
                log_im_change_approval(
                    &app,
                    "conflict",
                    &normalized_action,
                    Some(&change.id),
                    &channel_hash,
                    started_at,
                );
                return "变更已过期，请重新生成；本地文件未被覆盖。".to_owned();
            }
            log_im_change_approval(
                &app,
                "failed",
                &normalized_action,
                Some(&change.id),
                &channel_hash,
                started_at,
            );
            format!("处理变更失败：{}", logging::sanitize_log_text(&error))
        }
    }
}

/** 返回 IM 详情的截断 diff，正文只在用户显式请求时发送。 */
fn build_im_change_details(change: &ProposedChange) -> String {
    // 飞书按 UTF-8 字节计入消息体；这里同时控制中文场景的实际传输体积。
    const MAX_DIFF_CHARS: usize = 700;
    let original = change
        .original
        .chars()
        .take(MAX_DIFF_CHARS / 2)
        .collect::<String>();
    let next = change
        .next
        .chars()
        .take(MAX_DIFF_CHARS / 2)
        .collect::<String>();
    format!(
        "变更 {} 详情\n目标：{}\n类型：{}\n\n--- 原内容 ---\n{}\n\n+++ 建议内容 +++\n{}\n\n回复“确认 {}”写入，或“取消 {}”放弃。",
        short_change_code(&change.id), change.target_path, change.r#type, original, next,
        short_change_code(&change.id), short_change_code(&change.id)
    )
}

/** 生成稳定、可输入的变更短编号；完整 ID 仍只保存在本地会话中。 */
pub(crate) fn short_change_code(change_id: &str) -> String {
    change_id.chars().take(12).collect()
}

/** 记录 IM 审批审计，只写入变更 ID、通道 hash、动作和结果，不记录正文或外部原始身份。 */
fn log_im_change_approval(
    app: &AppHandle,
    status: &str,
    action: &str,
    change_id: Option<&str>,
    channel_hash: &str,
    started_at: Instant,
) {
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            if matches!(status, "failed" | "conflict") {
                AppLogLevel::Warn
            } else {
                AppLogLevel::Info
            },
            AppLogCategory::Im,
            "im_change_approval",
            status,
            "IM 待确认变更操作已处理。",
        )
        .entity("change", change_id.unwrap_or("unknown"))
        .duration(started_at.elapsed())
        .metadata(json!({ "action": action, "channelHash": channel_hash })),
    );
}

/** 为 IM channel 找到已有会话；不存在或失效时创建一个新的 AgentSession 并保存映射。 */
fn resolve_or_create_im_session(
    app: &AppHandle,
    snapshot: &mut WorkspaceSnapshot,
    channel_key: &str,
    im_identity: &crate::domain::ImSessionIdentity,
    knowledge_base_ids: Vec<String>,
) -> Result<ImSessionResolution, String> {
    let mapped_session_id = storage::load_im_session_mapping(app, channel_key)?;

    if let Some(session_id) = mapped_session_id {
        if let Some(session) = snapshot
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.knowledge_base_ids = knowledge_base_ids;
            session.updated_at = storage::format_local_datetime();
            let identity_status = if session.im_identity.is_some() {
                // 已有 IM 身份时只更新最近消息摘要，稳定标题不能被后续消息覆盖。
                if let Some(existing_identity) = &mut session.im_identity {
                    existing_identity.last_message_preview =
                        im_identity.last_message_preview.clone();
                }
                "updated"
            } else {
                // 旧版映射会话首次再次收到 IM 消息时补齐完整身份和标题。
                session.im_identity = Some(im_identity.clone());
                session.title = crate::im::format_im_session_title(im_identity);
                "migrated"
            };

            return Ok(ImSessionResolution {
                session_id: session.id.clone(),
                identity_status,
            });
        }
    }

    let session = crate::im::build_im_agent_session(im_identity.clone(), knowledge_base_ids);
    let session_id = session.id.clone();

    snapshot.sessions.insert(0, session);
    storage::save_im_session_mapping(app, channel_key, &session_id)?;

    Ok(ImSessionResolution {
        session_id,
        identity_status: "created",
    })
}

/** IM 会话创建、迁移和更新的结果；用于统一写入轻量脱敏身份审计。 */
struct ImSessionResolution {
    session_id: String,
    identity_status: &'static str,
}

/** 构造可观测但不包含消息正文和外部原始 ID 的 IM 身份日志。 */
fn build_im_identity_event(
    session_id: &str,
    identity: &crate::domain::ImSessionIdentity,
    status: &str,
) -> AppEventBuilder {
    AppEventBuilder::new(
        AppLogLevel::Info,
        AppLogCategory::Im,
        "im_session_identity",
        status,
        "IM 会话身份已同步。",
    )
    .session_id(session_id)
    .metadata(json!({
        "providerId": identity.provider_id,
        "conversationKind": identity.conversation_kind,
        "channelHash": identity.channel_hash,
        "initialPreviewChars": identity.initial_message_preview.chars().count(),
        "lastPreviewChars": identity.last_message_preview.chars().count(),
        "isFallback": identity.conversation_kind == "unknown",
    }))
}

/** 确认待写入 diff，校验知识库边界和内容 hash 后原子写回 Markdown。 */
#[tauri::command]
pub async fn apply_proposed_change(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let mut snapshot = payload.snapshot;
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

    if change.r#type == "create" {
        // 新建草稿不能覆盖用户已有文件；如路径已存在，应重新生成不同目标路径的 diff。
        if target_path.exists() {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Warn,
                    AppLogCategory::Agent,
                    "apply_proposed_change",
                    "blocked",
                    "目标 Markdown 已存在，已阻止覆盖。",
                )
                .operation_id(operation_id)
                .session_id(session_id)
                .knowledge_base_id(knowledge_base_id)
                .entity("change", change.id)
                .relative_path(change.target_path)
                .duration(started_at.elapsed()),
            );

            return Err("目标 Markdown 已存在，已阻止覆盖。请重新生成草稿路径。".to_owned());
        }

        let write_path = target_path.clone();
        let next_content = change.next.clone();

        run_blocking("写入新 Markdown 文件", move || {
            storage::atomic_write_markdown(&write_path, &next_content)
        })
        .await?;
        snapshot.notes.insert(
            0,
            crate::domain::Note {
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
    } else if let Some(note_id) = &change.note_id {
        let note_index = snapshot
            .notes
            .iter()
            .position(|note| note.id == *note_id)
            .ok_or_else(|| "找不到待写入笔记".to_owned())?;
        let read_path = target_path.clone();
        let fallback_content = snapshot.notes[note_index].content.clone();
        let current_content = run_blocking("读取待写入 Markdown 文件", move || {
            Ok(fs::read_to_string(&read_path).unwrap_or(fallback_content))
        })
        .await?;
        let current_hash = storage::hash_content(&current_content);

        let next_content = match apply_rewrite_change(
            &current_content,
            &current_hash,
            &snapshot.notes[note_index].content_hash,
            &change,
        ) {
            Ok(next_content) => next_content,
            Err(error) => {
                logging::write_app_event_best_effort(
                    &app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::Agent,
                        "apply_proposed_change",
                        "blocked",
                        error.clone(),
                    )
                    .operation_id(operation_id)
                    .session_id(session_id)
                    .knowledge_base_id(knowledge_base_id)
                    .entity("change", change.id)
                    .relative_path(change.target_path)
                    .duration(started_at.elapsed()),
                );

                return Err(error);
            }
        };
        let write_path = target_path.clone();
        let write_content = next_content.clone();

        capture_document_history_before_write(
            &app,
            storage::DocumentHistoryCapture {
                target_kind: "note".to_owned(),
                knowledge_base_id: knowledge_base_id.clone(),
                target_id: note_id.clone(),
                relative_path: snapshot.notes[note_index].path.clone(),
                title: snapshot.notes[note_index].title.clone(),
                file_type: "markdown".to_owned(),
                content: current_content,
                source: "agent-change".to_owned(),
                session_id: Some(session_id.clone()),
                change_id: Some(change.id.clone()),
                operation_id: Some(operation_id.clone()),
            },
            AppLogCategory::Agent,
            "apply_proposed_change",
            started_at,
        )
        .await?;

        run_blocking("写回 Markdown 文件", move || {
            storage::atomic_write_markdown(&write_path, &write_content)
        })
        .await?;
        snapshot.notes[note_index].content = next_content.clone();
        snapshot.notes[note_index].content_hash = storage::hash_content(&next_content);
        snapshot.notes[note_index].updated_at = "刚刚".to_owned();
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
    snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
        status: "accepted".to_owned(),
        ..change
    });
    runtime::update_agent_context_summary_deterministic(&mut snapshot, session_index, None, false);
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    logging::write_app_event_best_effort(
        &app,
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
fn apply_rewrite_change(
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

/** 拒绝待写入 diff，只更新会话状态，不修改任何 Markdown 文件。 */
#[tauri::command]
pub async fn reject_proposed_change(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let session_id = snapshot.active_session_id.clone();
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;

    if let Some(change) = snapshot.sessions[session_index].pending_change.clone() {
        let rejected_change_id = change.id.clone();
        let rejected_change_type = change.r#type.clone();
        let rejected_operation = change.operation.clone();
        let rejected_review_comment_count = change
            .review_comments
            .as_ref()
            .map(|comments| comments.len())
            .unwrap_or_default();
        let rejected_diff_hunk_count = change.diff_stats.as_ref().map(|stats| stats.hunk_count);
        let rejected_knowledge_base_id = change.knowledge_base_id.clone();
        let rejected_target_path = change.target_path.clone();

        snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
            status: "rejected".to_owned(),
            ..change
        });
        snapshot.sessions[session_index].updated_at = "刚刚".to_owned();
        runtime::update_agent_context_summary_deterministic(
            &mut snapshot,
            session_index,
            None,
            false,
        );

        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Agent,
                "reject_proposed_change",
                "completed",
                "已拒绝 Agent diff。",
            )
            .session_id(session_id)
            .knowledge_base_id(rejected_knowledge_base_id)
            .entity("change", rejected_change_id)
            .relative_path(rejected_target_path)
            .duration(started_at.elapsed())
            .metadata(json!({
                "changeType": rejected_change_type,
                "operation": rejected_operation,
                "reviewCommentCount": rejected_review_comment_count,
                "diffHunkCount": rejected_diff_hunk_count,
            })),
        );
    }

    index_snapshot_in_background(app.clone(), &snapshot).await?;

    Ok(snapshot)
}

/** Agent turn 前合并 SQLite 中的持久化会话，避免模型或规则 Agent 只信任前端传入的 scope 快照。 */
async fn hydrate_persisted_sessions_for_turn(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let sessions_app = app.clone();
    let snapshot_for_sessions = snapshot.clone();
    let persisted_sessions = run_blocking("读取持久化 Agent 会话", move || {
        storage::load_sessions_for_snapshot(&sessions_app, &snapshot_for_sessions)
    })
    .await?;

    if !persisted_sessions.is_empty() {
        snapshot.sessions = persisted_sessions;
    }

    storage::normalize_sessions_for_snapshot(&mut snapshot);

    if !snapshot
        .sessions
        .iter()
        .any(|session| session.id == snapshot.active_session_id)
    {
        snapshot.active_session_id = snapshot
            .sessions
            .first()
            .map(|session| session.id.clone())
            .unwrap_or_default();
    }

    Ok(snapshot)
}

/** 已准备好的安装来源，TempDir 持有临时目录生命周期直到后台安装结束。 */
enum PreparedSkillInstallSource {
    Borrowed {
        path: PathBuf,
        source_summary: String,
    },
    Temp {
        temp_dir: tempfile::TempDir,
        source_summary: String,
    },
}

impl PreparedSkillInstallSource {
    /** 返回统一安装管线可读取的根目录。 */
    fn root_path(&self) -> &Path {
        match self {
            PreparedSkillInstallSource::Borrowed { path, .. } => path.as_path(),
            PreparedSkillInstallSource::Temp { temp_dir, .. } => temp_dir.path(),
        }
    }

    /** 返回已脱敏的来源摘要，用于日志、UI 和安装元数据。 */
    fn source_summary(&self) -> &str {
        match self {
            PreparedSkillInstallSource::Borrowed { source_summary, .. }
            | PreparedSkillInstallSource::Temp { source_summary, .. } => source_summary,
        }
    }
}

/** 根据 payload 准备安装来源；本地来源未传路径时打开系统选择器。 */
async fn prepare_skill_install_source(
    app: &AppHandle,
    payload: &InstallAgentSkillPayload,
) -> Result<PreparedSkillInstallSource, String> {
    match payload.source_type.as_str() {
        "url" => {
            prepare_url_skill_install_source(payload.source.as_deref().unwrap_or_default()).await
        }
        "localFolder" => {
            let path = match payload
                .source
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                Some(path) => PathBuf::from(path),
                None => pick_skill_folder(app).await?,
            };

            if !path.exists() || !path.is_dir() {
                return Err("请选择有效的 Skill 文件夹。".to_owned());
            }

            Ok(PreparedSkillInstallSource::Borrowed {
                source_summary: summarize_local_install_source(&path),
                path,
            })
        }
        "localArchive" => {
            let path = match payload
                .source
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                Some(path) => PathBuf::from(path),
                None => pick_skill_archive(app).await?,
            };

            if !path.exists() || !path.is_file() {
                return Err("请选择有效的 Skill zip 文件。".to_owned());
            }

            let bytes = read_limited_file(&path, skills::MAX_REMOTE_SKILL_ARCHIVE_BYTES)?;
            let temp_dir = skills::prepare_skill_archive_bytes(&bytes)?;

            Ok(PreparedSkillInstallSource::Temp {
                source_summary: summarize_local_install_source(&path),
                temp_dir,
            })
        }
        _ => Err("未知的 Skill 安装来源类型。".to_owned()),
    }
}

/** 下载远程 Skill 来源并转换成统一的临时目录。 */
async fn prepare_url_skill_install_source(url: &str) -> Result<PreparedSkillInstallSource, String> {
    let download = skills::resolve_skill_url_download(url)?;
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| format!("无法创建 Skill 下载客户端：{error}"))?
        .get(&download.url)
        .header(
            reqwest::header::ACCEPT,
            "text/markdown, application/zip, */*",
        )
        .send()
        .await
        .map_err(|error| format!("下载 Skill 失败：{error}"))?;

    if !response.status().is_success() {
        return Err(format!("下载 Skill 失败：HTTP {}", response.status()));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    let is_archive_download = matches!(download.kind, skills::SkillUrlDownloadKind::Archive)
        || content_type.contains("zip")
        || download.url.ends_with(".zip");
    let max_bytes = if is_archive_download {
        skills::MAX_REMOTE_SKILL_ARCHIVE_BYTES
    } else {
        skills::MAX_REMOTE_SKILL_MARKDOWN_BYTES
    };
    let bytes = read_limited_response_bytes(response, max_bytes, is_archive_download).await?;
    let temp_dir = if is_archive_download {
        skills::prepare_skill_archive_bytes(&bytes)?
    } else {
        let markdown = String::from_utf8(bytes)
            .map_err(|_| "远程 Skill 内容不是有效 UTF-8 文本。".to_owned())?;

        skills::prepare_single_skill_markdown(&markdown)?
    };

    Ok(PreparedSkillInstallSource::Temp {
        source_summary: download.source_summary,
        temp_dir,
    })
}

/** 按最大字节数读取远程响应体，Content-Length 缺失时也能在流式读取过程中截断。 */
async fn read_limited_response_bytes(
    mut response: reqwest::Response,
    max_bytes: usize,
    is_archive: bool,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_bytes as u64)
    {
        return Err(remote_skill_size_limit_message(is_archive));
    }

    let mut bytes = Vec::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取 Skill 下载内容失败：{error}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(remote_skill_size_limit_message(is_archive));
        }

        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

/** 返回远程下载大小限制提示，避免多个下载分支各自硬编码文案。 */
fn remote_skill_size_limit_message(is_archive: bool) -> String {
    if is_archive {
        "远程 Skill 压缩包超过 25MB，已阻止安装。".to_owned()
    } else {
        "远程 SKILL.md 超过 1MB，已阻止安装。".to_owned()
    }
}

/** 打开系统目录选择器选择待安装 Skill 文件夹。 */
async fn pick_skill_folder(app: &AppHandle) -> Result<PathBuf, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择 Skill 文件夹")
        .pick_folder(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    receiver
        .recv()
        .await
        .flatten()
        .and_then(|path| path.as_path().map(PathBuf::from))
        .ok_or_else(|| "未选择 Skill 文件夹。".to_owned())
}

/** 打开系统文件选择器选择待安装 Skill zip。 */
async fn pick_skill_archive(app: &AppHandle) -> Result<PathBuf, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);

    app.dialog()
        .file()
        .set_title("选择 Skill zip 文件")
        .add_filter("Zip archive", &["zip"])
        .pick_file(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    receiver
        .recv()
        .await
        .flatten()
        .and_then(|path| path.as_path().map(PathBuf::from))
        .ok_or_else(|| "未选择 Skill zip 文件。".to_owned())
}

/** 读取本地压缩包并限制最大字节数，避免大文件通过 IPC 之外的路径阻塞安装。 */
fn read_limited_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("无法读取 Skill 文件元数据：{error}"))?;

    if metadata.len() > max_bytes as u64 {
        return Err("Skill zip 文件超过 25MB，已阻止安装。".to_owned());
    }

    let mut file =
        fs::File::open(path).map_err(|error| format!("无法读取 Skill zip 文件：{error}"))?;
    let mut bytes = Vec::new();

    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 Skill zip 文件：{error}"))?;

    if bytes.len() > max_bytes {
        return Err("Skill zip 文件超过 25MB，已阻止安装。".to_owned());
    }

    Ok(bytes)
}

/** 生成本地安装来源摘要，只保留文件或目录名，避免日志写入绝对路径。 */
fn summarize_local_install_source(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| format!("local:{name}"))
        .unwrap_or_else(|| "local".to_owned())
}

/** 在 Tauri 后台阻塞线程中运行文件系统或 SQLite 重任务，避免卡住 WebView 主线程。 */
async fn run_blocking<T, F>(label: &str, task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| format!("{label}后台任务失败：{error}"))?
}

/** 使用系统文件管理器打开目录；失败时返回命令层错误，前端可展示路径供用户手动访问。 */
fn open_folder_in_system(path: &Path) -> Result<(), String> {
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
async fn index_snapshot_in_background(
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

/** 统计目录中的 Markdown 文件数量，用于目录选择后的即时反馈。 */
fn count_markdown_files(root: &PathBuf) -> Result<usize, String> {
    let mut count = 0;

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(storage::should_walk_entry)
        .filter_map(Result::ok)
    {
        let path = entry.path();

        // 只统计 Markdown 文件；真实扫描阶段会进一步解析标题、标签和正文。
        if path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("md") | Some("markdown")
            )
        {
            count += 1;
        }
    }

    Ok(count)
}

/** 从新建文件相对路径提取初始标题，空白正文会在重扫时继续使用文件名。 */
fn note_title_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名")
        .to_owned()
}

/** 从普通文档相对路径提取标题，txt/docx/pdf/图片首版都使用文件名 stem。 */
fn document_title_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名文档")
        .to_owned()
}

/** 从目录相对路径取最后一级名称，用于 create_folder 后立即生成前端目录节点。 */
fn folder_name_from_path(relative_path: &str) -> String {
    Path::new(relative_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名目录")
        .to_owned()
}

/** 重命名后把活跃笔记、固定笔记和待确认 diff 中的旧 note id 迁移到新 id。 */
fn replace_note_reference_after_rename(
    snapshot: &mut WorkspaceSnapshot,
    previous_note_id: &str,
    next_note_id: &str,
    next_relative_path: &str,
) {
    if snapshot.active_note_id == previous_note_id {
        snapshot.active_note_id = next_note_id.to_owned();
    }

    for session in &mut snapshot.sessions {
        if session.active_note_id.as_deref() == Some(previous_note_id) {
            session.active_note_id = Some(next_note_id.to_owned());
        }

        for pinned_note_id in &mut session.pinned_note_ids {
            if pinned_note_id == previous_note_id {
                *pinned_note_id = next_note_id.to_owned();
            }
        }
        session.pinned_note_ids.sort();
        session.pinned_note_ids.dedup();

        if let Some(change) = &mut session.pending_change {
            if change.note_id.as_deref() == Some(previous_note_id) {
                change.note_id = Some(next_note_id.to_owned());
                change.target_path = next_relative_path.to_owned();
            }
        }
    }
}

/** 删除后清理会话中的笔记引用和待确认 diff，避免 UI 指向已移入回收站的文件。 */
fn remove_note_references_after_delete(snapshot: &mut WorkspaceSnapshot, note_id: &str) {
    if snapshot.active_note_id == note_id {
        snapshot.active_note_id.clear();
    }

    for session in &mut snapshot.sessions {
        if session.active_note_id.as_deref() == Some(note_id) {
            session.active_note_id = None;
        }

        session.pinned_note_ids.retain(|id| id != note_id);

        if session
            .pending_change
            .as_ref()
            .is_some_and(|change| change.note_id.as_deref() == Some(note_id))
        {
            session.pending_change = None;
        }
    }
}

/** 规范知识库默认标记，保证列表中最多只有第一项是默认知识库。 */
fn normalize_knowledge_base_flags(snapshot: &mut WorkspaceSnapshot) {
    for (index, knowledge_base) in snapshot.knowledge_bases.iter_mut().enumerate() {
        knowledge_base.is_default = index == 0;
    }
}

/** 修正活跃知识库、笔记和会话，避免扫描、移除后工作台指向不存在的对象。 */
fn normalize_active_entities(
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

/** 重扫后清理会话中已经不存在的笔记引用，避免上下文指向旧文件。 */
fn normalize_sessions_after_rescan(snapshot: &mut WorkspaceSnapshot, knowledge_base_id: &str) {
    let note_ids: std::collections::HashSet<String> =
        snapshot.notes.iter().map(|note| note.id.clone()).collect();

    for session in &mut snapshot.sessions {
        // 只有绑定目标知识库的会话需要修正；多知识库会话中的其他有效笔记引用必须保留。
        if !session
            .knowledge_base_ids
            .iter()
            .any(|id| id == knowledge_base_id)
        {
            continue;
        }

        if session
            .active_note_id
            .as_ref()
            .is_some_and(|active_note_id| !note_ids.contains(active_note_id))
        {
            session.active_note_id = None;
        }

        session
            .pinned_note_ids
            .retain(|note_id| note_ids.contains(note_id));

        if session
            .pending_change
            .as_ref()
            .and_then(|change| change.note_id.as_ref())
            .is_some_and(|note_id| !note_ids.contains(note_id))
        {
            session.pending_change = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AgentSession, KnowledgeBase, Note};

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

        let next_content =
            apply_rewrite_change(current_content, &current_hash, &current_hash, &change).unwrap();

        assert_eq!(next_content, "开头\n新段落\n结尾");
    }

    /** 当前文件中原文片段出现多次时必须拒绝写入，避免一次确认误改多处。 */
    #[test]
    fn apply_rewrite_change_rejects_ambiguous_original() {
        let current_content = "旧段落\n中间\n旧段落";
        let current_hash = storage::hash_content(current_content);
        let change = test_rewrite_change("旧段落", "新段落", &current_hash);

        let result = apply_rewrite_change(current_content, &current_hash, &current_hash, &change);

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

        let result = apply_rewrite_change(
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

        let next_content =
            apply_rewrite_change(current_content, &current_hash, &current_hash, &change).unwrap();

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

        let result = apply_rewrite_change(current_content, &current_hash, &snapshot_hash, &change);

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

        let next_content =
            apply_rewrite_change(current_content, &current_hash, &current_hash, &change).unwrap();

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

        let result = apply_rewrite_change(current_content, &current_hash, &snapshot_hash, &change);

        assert_eq!(
            result.unwrap_err(),
            "目标文件已变化，已阻止多处编辑写入。请重新生成 diff。"
        );
    }
}
