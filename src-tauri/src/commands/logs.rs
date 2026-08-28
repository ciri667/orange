use super::common::*;

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

/** 读取某会话最近一次发给模型的上下文预览；没有转储时返回 null。 */
#[tauri::command]
pub async fn load_agent_prompt_dump(
    app: AppHandle,
    payload: LoadAgentPromptDumpPayload,
) -> Result<Option<crate::domain::AgentPromptDump>, String> {
    run_blocking("读取最近一次模型上下文", move || {
        logging::load_agent_prompt_dump(&app, &payload.session_id)
    })
    .await
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
