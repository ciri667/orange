use crate::domain::{
    AgentSession, AgentSkill, AgentTurnPayload, AgentTurnResult, AppEventLog, ChangePayload,
    CreateDocumentPayload, CreateFolderPayload, CreateNotePayload, DeleteAgentSkillPayload,
    DeleteDocumentPayload, DeleteNotePayload, DeleteSessionPayload, DocumentPreview, FolderEntry,
    InstallAgentSkillPayload, InstallAgentSkillResult, KnowledgeBaseSelection,
    LoadAppEventLogsPayload, LoadDocumentPreviewPayload, LoadSessionsPayload, ModelApiKeyStatus,
    ProposedChange, RemoveKnowledgeBasePayload, RenameDocumentPayload, RenameNotePayload,
    RequestAuditLog, RescanKnowledgeBasePayload, RestoreSessionContextPayload,
    SaveAgentSkillPayload, SaveDocumentContentPayload, SaveModelApiKeyPayload,
    SaveNoteContentPayload, SaveNoteImageAttachmentsPayload, SaveSessionPayload,
    SaveUserSettingsPayload, ScanKnowledgeBasePayload, ScanReport, ToggleAgentSkillPayload,
    UpdateSessionScopePayload, UserSettings, WorkspaceSnapshot,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::runtime;
use crate::skills;
use crate::storage;
use crate::text_edit::{replace_unique, UniqueReplacementError};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

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

/** 读取持久化 Agent 会话，并按当前工作台快照清理已失效的知识库或笔记引用。 */
#[tauri::command]
pub async fn load_sessions(
    app: AppHandle,
    payload: LoadSessionsPayload,
) -> Result<Vec<AgentSession>, String> {
    run_blocking("读取 Agent 会话", move || {
        storage::load_sessions_for_snapshot(&app, &payload.snapshot)
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
    let model_name = saved_settings.model_config.model.clone();

    let result = run_blocking("保存用户设置", move || {
        storage::save_user_settings(&settings_app, &saved_settings)?;
        Ok(saved_settings)
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
            .metadata(json!({ "modelEnabled": model_enabled, "model": model_name })),
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

/** 读取内置和用户自建 skills，内置定义会合并用户保存的启停偏好。 */
#[tauri::command]
pub async fn load_agent_skills(app: AppHandle) -> Result<Vec<AgentSkill>, String> {
    run_blocking("读取 Skills", move || {
        let connection = storage::open_database(&app)?;

        skills::load_agent_skills(&app, &connection)
    })
    .await
}

/** 打开 Cici Note 用户 Skills 文件夹，浏览器开发态由前端 mock 只展示路径。 */
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

/** 启停 skill；allowAutoInvoke 仅兼容旧客户端 payload。 */
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

        skills::toggle_agent_skill(
            &skills_app,
            &connection,
            &payload.skill_id,
            payload.enabled,
            payload.allow_auto_invoke,
        )
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

/** 保存 BYOK 模型密钥到系统安全存储，SQLite 只保存 keyReference。 */
#[tauri::command]
pub async fn save_model_api_key(
    app: AppHandle,
    payload: SaveModelApiKeyPayload,
) -> Result<ModelApiKeyStatus, String> {
    let started_at = Instant::now();
    let result = run_blocking("保存模型密钥", move || {
        storage::save_model_api_key(&payload.api_key)
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
            .metadata(json!({ "configured": status.configured, "keyReference": status.key_reference.clone() })),
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
            .duration(started_at.elapsed()),
        ),
    }

    result
}

/** 读取 BYOK 模型密钥状态，只返回是否已配置，不返回明文。 */
#[tauri::command]
pub async fn load_model_api_key_status() -> Result<ModelApiKeyStatus, String> {
    run_blocking("读取模型密钥状态", storage::load_model_api_key_status).await
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
    let new_note = crate::domain::Note {
        id: note_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: note_title_from_path(&relative_path),
        path: relative_path,
        content: String::new(),
        tags: Vec::new(),
        updated_at: "刚刚".to_owned(),
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
    let new_document = crate::domain::WorkspaceDocument {
        id: document_id.clone(),
        knowledge_base_id: knowledge_base.id.clone(),
        title: document_title_from_path(&relative_path),
        path: relative_path,
        file_type: "txt".to_owned(),
        updated_at: "刚刚".to_owned(),
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

    snapshot.notes[note_index].id = next_note_id.clone();
    snapshot.notes[note_index].title = next_title;
    snapshot.notes[note_index].path = next_relative_path.clone();
    snapshot.notes[note_index].content = current_content;
    snapshot.notes[note_index].content_hash = current_hash;
    snapshot.notes[note_index].updated_at = "刚刚".to_owned();
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

    snapshot.documents[document_index].id = next_document_id.clone();
    snapshot.documents[document_index].title = document_title_from_path(&next_relative_path);
    snapshot.documents[document_index].path = next_relative_path;
    snapshot.documents[document_index].content = Some(current_content);
    snapshot.documents[document_index].content_hash = current_hash;
    snapshot.documents[document_index].updated_at = "刚刚".to_owned();

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

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 Markdown 文件", move || {
        storage::atomic_write_markdown(&write_path, &write_content)
    })
    .await?;

    let next_hash = storage::hash_content(&payload.content);
    snapshot.notes[note_index].content = payload.content;
    snapshot.notes[note_index].content_hash = next_hash;
    snapshot.notes[note_index].updated_at = "刚刚".to_owned();
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

    let write_path = target_path.clone();
    let write_content = payload.content.clone();

    run_blocking("保存 TXT 文件", move || {
        storage::atomic_write_text_document(&write_path, &write_content)
    })
    .await?;

    let next_hash = storage::hash_content(&payload.content);
    snapshot.documents[document_index].content = Some(payload.content);
    snapshot.documents[document_index].content_hash = next_hash;
    snapshot.documents[document_index].updated_at = "刚刚".to_owned();
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

/** 加载 docx/pdf 文档预览，命令层负责定位知识库并把路径授权给 asset protocol。 */
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
    let mut request_metadata = json!({ "action": request.action.clone() });

    if let Some(selected_skill_id) = request.selected_skill_id.clone() {
        request_metadata["selectedSkillId"] = json!(selected_skill_id);
    }

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
    let accepted_target_path = change.target_path.clone();
    snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
        status: "accepted".to_owned(),
        ..change
    });
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
        .metadata(json!({ "changeType": accepted_change_type })),
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
        let rejected_knowledge_base_id = change.knowledge_base_id.clone();
        let rejected_target_path = change.target_path.clone();

        snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
            status: "rejected".to_owned(),
            ..change
        });
        snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

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
            .metadata(json!({ "changeType": rejected_change_type })),
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

/** 从普通文档相对路径提取标题，txt/docx/pdf 首版都使用文件名 stem。 */
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
                title: "改写 note-b".to_owned(),
                target_path: "note-b.md".to_owned(),
                original: "旧内容".to_owned(),
                next: "新内容".to_owned(),
                original_hash: storage::hash_content("旧内容"),
                status: "pending".to_owned(),
            }),
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
            deleted_at: None,
        }
    }

    /** 构造可直接喂给 apply_rewrite_change 的待确认改写。 */
    fn test_rewrite_change(original: &str, next: &str, original_hash: &str) -> ProposedChange {
        ProposedChange {
            id: "change-test".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            note_id: Some("note-a".to_owned()),
            r#type: "rewrite".to_owned(),
            title: "改写 note-a".to_owned(),
            target_path: "note-a.md".to_owned(),
            original: original.to_owned(),
            next: next.to_owned(),
            original_hash: original_hash.to_owned(),
            status: "pending".to_owned(),
        }
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
}
