use super::common::run_blocking;
use crate::{domain::*, retrieval, storage};
use serde::Deserialize;
use tauri::{AppHandle, Emitter};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSemanticSettingsPayload {
    settings: SemanticSearchSettings,
    #[serde(default)]
    allow_cloud_upload: bool,
}

/** 显式云端授权只在此命令产生，普通 UserSettings 保存不能伪造或沿用授权。 */
#[tauri::command]
pub async fn save_semantic_search_settings(
    app: AppHandle,
    payload: SaveSemanticSettingsPayload,
) -> Result<UserSettings, String> {
    run_blocking("保存语义检索设置", move || {
        let mut settings = storage::load_user_settings(&app)?;
        let mut semantic = payload.settings;
        semantic.api_base = semantic.api_base.trim().trim_end_matches('/').to_owned();
        semantic.model = semantic.model.trim().to_owned();
        if semantic.enabled {
            retrieval::provider::validate(&semantic)?;
        }
        let fingerprint = retrieval::provider::fingerprint(&semantic);
        let remote = retrieval::provider::endpoint(&semantic)?.1;
        let consent = settings
            .semantic_search
            .cloud_consent_fingerprint
            .as_deref()
            == Some(fingerprint.as_str());
        semantic.cloud_consent_fingerprint = if remote && (payload.allow_cloud_upload || consent) {
            Some(fingerprint)
        } else {
            None
        };
        settings.semantic_search = semantic;
        if settings.semantic_search.enabled {
            storage::semantic_allowed(&settings)?;
        }
        // 授权与配置在同一个写事务中持久化；旧响应随后会因指纹或代次不匹配被拒绝。
        storage::persist_semantic_settings(&app, &settings)?;
        Ok(settings)
    })
    .await
}

#[tauri::command]
pub async fn load_semantic_index_statuses(
    app: AppHandle,
) -> Result<Vec<SemanticIndexStatus>, String> {
    run_blocking("读取语义索引进度", move || {
        storage::load_semantic_index_statuses(&app)
    })
    .await
}

#[tauri::command]
pub async fn control_semantic_index(
    app: AppHandle,
    payload: SemanticIndexAction,
) -> Result<Vec<SemanticIndexStatus>, String> {
    let event_app = app.clone();
    let statuses = run_blocking("管理语义索引", move || {
        storage::apply_semantic_index_action(&app, &payload)
    })
    .await?;
    let _ = event_app.emit("semantic-index-progress", &statuses);
    Ok(statuses)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticKeyPayload {
    settings: SemanticSearchSettings,
    api_key: String,
}

#[tauri::command]
pub async fn save_semantic_api_key(
    payload: SemanticKeyPayload,
) -> Result<SemanticCredentialStatus, String> {
    run_blocking("保存向量服务密钥", move || {
        retrieval::provider::endpoint(&payload.settings)?;
        if payload.api_key.trim().is_empty() {
            return Err("密钥不能为空。".into());
        }
        let id = format!(
            "semantic-{}",
            retrieval::provider::fingerprint(&payload.settings)
        );
        storage::save_model_api_key(&id, payload.api_key.trim())
            .map_err(|_| "无法保存向量服务密钥。".to_owned())?;
        Ok(SemanticCredentialStatus { configured: true })
    })
    .await
}

#[tauri::command]
pub async fn load_semantic_credential_status(
    settings: SemanticSearchSettings,
) -> Result<SemanticCredentialStatus, String> {
    run_blocking("读取向量服务密钥状态", move || {
        let configured = storage::load_model_api_key(&storage::semantic_key_reference(&settings))
            .map_err(|_| "无法读取向量服务密钥状态。")?
            .is_some_and(|key| !key.trim().is_empty());
        Ok(SemanticCredentialStatus { configured })
    })
    .await
}

/** 探测只发送固定文本；即使尚未授权知识库上传，也不会读取任何笔记。 */
#[tauri::command]
pub async fn probe_semantic_search(
    app: AppHandle,
    settings: SemanticSearchSettings,
) -> Result<SemanticProbeResult, String> {
    retrieval::provider::validate(&settings)?;
    if retrieval::provider::endpoint(&settings)?.1
        && storage::load_user_settings(&app)?.privacy_policy != "allow-selected-scope"
    {
        return Err("仅本地隐私策略禁止云端请求。".into());
    }
    let key_settings = settings.clone();
    let key = run_blocking("读取向量服务密钥", move || {
        storage::load_semantic_key(&key_settings)
    })
    .await?;
    let vectors = retrieval::provider::embed(
        &settings,
        &key,
        &["橘记语义检索连接测试".into()],
        std::time::Duration::from_secs(60),
    )
    .await
    .map_err(|e| e.message.to_owned())?;
    Ok(SemanticProbeResult {
        dimensions: vectors[0].len(),
        message: "连接成功，模型可以生成向量。".into(),
    })
}
