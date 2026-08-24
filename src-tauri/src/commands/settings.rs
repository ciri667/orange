use super::common::*;

/** 模型列表刷新最多保存的条目数，避免 OpenRouter 等聚合平台把设置 JSON 撑得过大。 */
pub(super) const MAX_REFRESHED_LLM_MODELS: usize = 500;

/** 模型列表刷新超时时间；短于对话请求，保证设置页操作可快速失败重试。 */
pub(super) const MODEL_LIST_HTTP_TIMEOUT_SECONDS: u64 = 20;

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
pub(super) async fn send_model_list_request(
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
