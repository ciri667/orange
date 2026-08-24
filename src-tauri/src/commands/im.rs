use super::common::*;

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
