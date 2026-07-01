use crate::domain::{LlmProviderConfig, ModelConfig, UserSettings};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/** 首版只支持 OpenAI-compatible 协议；Anthropic/Gemini 原生适配留作后续 todo。 */
pub const DEFAULT_PROVIDER_TYPE: &str = "openai-compatible";

/** 迁移旧版单 provider 配置时固定使用的 provider id，确保沿用旧 keyReference。 */
pub const MIGRATED_DEFAULT_PROVIDER_ID: &str = "default";

/** 模型错误文本脱敏后的最大字符数，避免整段响应正文进入可见消息或日志。 */
const MAX_REDACTED_TEXT_CHARS: usize = 500;

/** 设置页“新增 Provider”入口使用的预置模板。 */
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplate {
    pub template_id: String,
    pub name: String,
    pub provider: String,
    pub api_base: String,
    pub model: String,
    pub requires_api_key: bool,
}

/** 内置 provider 模板：OpenAI、DeepSeek、OpenRouter、Ollama（本地免鉴权）和自定义兼容服务。 */
pub fn provider_templates() -> Vec<ProviderTemplate> {
    vec![
        ProviderTemplate {
            template_id: "openai".to_owned(),
            name: "OpenAI".to_owned(),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: "https://api.openai.com/v1".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            requires_api_key: true,
        },
        ProviderTemplate {
            template_id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: "https://api.deepseek.com/v1".to_owned(),
            model: "deepseek-chat".to_owned(),
            requires_api_key: true,
        },
        ProviderTemplate {
            template_id: "openrouter".to_owned(),
            name: "OpenRouter".to_owned(),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: "https://openrouter.ai/api/v1".to_owned(),
            model: "openai/gpt-4o-mini".to_owned(),
            requires_api_key: true,
        },
        ProviderTemplate {
            template_id: "ollama".to_owned(),
            name: "Ollama（本地）".to_owned(),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: "http://localhost:11434/v1".to_owned(),
            model: "llama3.1".to_owned(),
            requires_api_key: false,
        },
        ProviderTemplate {
            template_id: "custom".to_owned(),
            name: "自定义兼容服务".to_owned(),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: String::new(),
            model: String::new(),
            requires_api_key: true,
        },
    ]
}

/** 模型选择或校验失败时的可见错误；Runtime 据此生成 model_error_turn，不会静默切换模型。 */
#[derive(Clone, Debug)]
pub struct ModelResolutionError(pub String);

impl std::fmt::Display for ModelResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/** 按“本轮 > 会话默认 > 全局默认”解析出实际使用的 Provider 配置。 */
pub fn resolve_provider<'a>(
    model_config: &'a ModelConfig,
    session_provider_id: Option<&str>,
    request_provider_id: Option<&str>,
) -> Result<&'a LlmProviderConfig, ModelResolutionError> {
    if !model_config.enabled {
        return Err(ModelResolutionError("云端模型未启用。".to_owned()));
    }

    fn non_empty(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    let selected_id = non_empty(request_provider_id)
        .or_else(|| non_empty(session_provider_id))
        .unwrap_or(model_config.default_provider_id.as_str());

    let provider = model_config
        .providers
        .iter()
        .find(|candidate| candidate.id == selected_id)
        .ok_or_else(|| ModelResolutionError(format!("未找到 Provider 配置：{selected_id}")))?;

    if !provider.enabled {
        return Err(ModelResolutionError(format!(
            "Provider「{}」已停用，请在设置中启用或切换 Provider。",
            provider.name
        )));
    }

    Ok(provider)
}

/** 拼接 OpenAI-compatible chat completions endpoint，兼容 base 已包含完整路径的情况。 */
pub fn chat_completions_endpoint(api_base: &str) -> String {
    let trimmed_base = api_base.trim_end_matches('/');

    if trimmed_base.ends_with("/chat/completions") {
        trimmed_base.to_owned()
    } else {
        format!("{trimmed_base}/chat/completions")
    }
}

/** 从完整 endpoint 中提取用于日志的 host，避免记录自定义部署的完整路径。 */
pub fn endpoint_host(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown-host".to_owned())
}

/** 按 providerId 生成系统安全存储的 key 引用；迁移出的默认 provider 沿用旧固定引用。 */
pub fn key_reference_for_provider(provider_id: &str) -> String {
    if provider_id == MIGRATED_DEFAULT_PROVIDER_ID {
        crate::storage::MODEL_KEY_REFERENCE.to_owned()
    } else {
        format!("cici-note-llm-provider-{provider_id}-api-key")
    }
}

/** 强制让每个 provider 的 key_reference 与其 id 保持确定性映射。
 *
 * `key_reference` 只应是 providerId 的派生值，不能由前端自由写入：
 * `save_model_api_key` 始终按 `key_reference_for_provider(provider_id)` 写入 keyring，
 * 如果配置里保存的 `key_reference` 和这个派生值不一致（例如前端新增 provider 时生成了
 * 随机占位引用），密钥保存和读取会分别落在两个不同的 keyring 条目上，
 * 导致“已保存密钥”之后仍然提示“未找到模型密钥”。这里在读取和保存设置时都做一次归一化，
 * 从根本上消除这种漂移，而不是在各个读取点分别硬编码兼容逻辑。 */
pub fn normalize_model_config_key_references(model_config: &mut ModelConfig) {
    for provider in &mut model_config.providers {
        provider.key_reference = key_reference_for_provider(&provider.id);
    }
}

/** 脱敏模型错误文本：移除常见密钥片段（Bearer/Authorization/sk-*）并限制长度。 */
pub fn redact_model_error_text(input: &str) -> String {
    let truncated_source: String = input.chars().take(4_000).collect();
    let lower_source = truncated_source.to_ascii_lowercase();
    let markers: [&str; 3] = ["bearer ", "authorization", "sk-"];
    let mut result = String::with_capacity(truncated_source.len());
    let mut cursor = 0usize;

    while cursor < truncated_source.len() {
        let next_marker = markers
            .iter()
            .filter_map(|marker| {
                lower_source[cursor..]
                    .find(marker)
                    .map(|offset| (cursor + offset, marker.len()))
            })
            .min_by_key(|(offset, _)| *offset);

        let Some((marker_start, marker_len)) = next_marker else {
            result.push_str(&truncated_source[cursor..]);
            break;
        };

        result.push_str(&truncated_source[cursor..marker_start]);
        result.push_str("[redacted]");

        let mut secret_end = marker_start + marker_len;
        while secret_end < truncated_source.len() {
            let remaining_char = truncated_source[secret_end..].chars().next().unwrap();

            if remaining_char.is_whitespace() || matches!(remaining_char, '"' | '\'' | ',' | '}') {
                break;
            }

            secret_end += remaining_char.len_utf8();
        }

        cursor = secret_end;
    }

    if result.chars().count() > MAX_REDACTED_TEXT_CHARS {
        let clipped: String = result.chars().take(MAX_REDACTED_TEXT_CHARS).collect();

        format!("{clipped}…（已截断）")
    } else {
        result
    }
}

/** 旧版单 provider 模型配置结构，仅在迁移历史 JSON 时使用。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyModelConfig {
    #[serde(default)]
    api_base: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    enabled: bool,
}

/** 旧版用户设置结构，仅在迁移历史 JSON 时使用。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyUserSettings {
    model_config: LegacyModelConfig,
    privacy_policy: String,
    #[serde(default)]
    write_confirmation_required: bool,
}

/** 判断设置 payload 是否是旧版单 provider 结构；只有确认缺少 providers 集合时才允许迁移。 */
fn is_legacy_user_settings_payload(payload_json: &str) -> Result<bool, String> {
    let payload: Value =
        serde_json::from_str(payload_json).map_err(|error| format!("无法解析用户设置：{error}"))?;
    let model_config = payload
        .get("modelConfig")
        .ok_or_else(|| "无法解析用户设置：缺少 modelConfig。".to_owned())?;

    // 新格式一旦带有 providers 字段，后续错误都应暴露为解析失败，不能误走旧格式迁移丢配置。
    Ok(!model_config.get("providers").is_some())
}

/** 解析用户设置 JSON；新格式直接反序列化，旧的单 provider 格式自动迁移成 provider 集合。 */
pub fn parse_or_migrate_user_settings_json(
    payload_json: &str,
    now: &str,
) -> Result<UserSettings, String> {
    match serde_json::from_str::<UserSettings>(payload_json) {
        Ok(mut settings) => {
            // 归一化 key_reference，自动修复历史上曾经写入过不一致引用的设置记录。
            normalize_model_config_key_references(&mut settings.model_config);

            return Ok(settings);
        }
        Err(new_format_error) if !is_legacy_user_settings_payload(payload_json)? => {
            return Err(format!("无法解析用户设置：{new_format_error}"));
        }
        Err(_) => {}
    }

    let legacy: LegacyUserSettings =
        serde_json::from_str(payload_json).map_err(|error| format!("无法解析用户设置：{error}"))?;

    Ok(migrate_legacy_user_settings(legacy, now))
}

/** 把旧的单 provider 配置包装成默认 provider，默认 provider 沿用旧 keyReference 避免迁移用户密钥。 */
fn migrate_legacy_user_settings(legacy: LegacyUserSettings, now: &str) -> UserSettings {
    let migrated_provider = LlmProviderConfig {
        id: MIGRATED_DEFAULT_PROVIDER_ID.to_owned(),
        name: "默认 Provider".to_owned(),
        provider: DEFAULT_PROVIDER_TYPE.to_owned(),
        api_base: legacy.model_config.api_base,
        model: legacy.model_config.model,
        key_reference: key_reference_for_provider(MIGRATED_DEFAULT_PROVIDER_ID),
        enabled: legacy.model_config.enabled,
        supports_tools: true,
        requires_api_key: true,
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    };

    UserSettings {
        model_config: ModelConfig {
            enabled: legacy.model_config.enabled,
            default_provider_id: MIGRATED_DEFAULT_PROVIDER_ID.to_owned(),
            providers: vec![migrated_provider],
        },
        privacy_policy: legacy.privacy_policy,
        write_confirmation_required: legacy.write_confirmation_required,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LlmProviderConfig;

    /** 构造测试用 Provider，默认已启用并要求 API key。 */
    fn test_provider(id: &str, enabled: bool) -> LlmProviderConfig {
        LlmProviderConfig {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            provider: DEFAULT_PROVIDER_TYPE.to_owned(),
            api_base: "https://llm.example/v1".to_owned(),
            model: "test-model".to_owned(),
            key_reference: key_reference_for_provider(id),
            enabled,
            supports_tools: true,
            requires_api_key: true,
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
        }
    }

    /** 构造包含两个 provider 的测试模型配置。 */
    fn test_model_config() -> ModelConfig {
        ModelConfig {
            enabled: true,
            default_provider_id: "provider-a".to_owned(),
            providers: vec![
                test_provider("provider-a", true),
                test_provider("provider-b", true),
            ],
        }
    }

    /** 旧版单 provider JSON 必须迁移成沿用旧 keyReference 的默认 provider。 */
    #[test]
    fn migrates_legacy_settings_and_preserves_key_reference() {
        let legacy_json = r#"{
            "modelConfig": {
                "provider": "openai-compatible",
                "apiBase": "https://api.openai.com/v1",
                "model": "gpt-4o-mini",
                "keyReference": "cici-note-openai-compatible-api-key",
                "enabled": true
            },
            "privacyPolicy": "allow-selected-scope",
            "writeConfirmationRequired": true
        }"#;

        let migrated = parse_or_migrate_user_settings_json(legacy_json, "刚刚").unwrap();

        assert!(migrated.model_config.enabled);
        assert_eq!(
            migrated.model_config.default_provider_id,
            MIGRATED_DEFAULT_PROVIDER_ID
        );
        assert_eq!(migrated.model_config.providers.len(), 1);
        let provider = &migrated.model_config.providers[0];
        assert_eq!(provider.id, MIGRATED_DEFAULT_PROVIDER_ID);
        assert_eq!(provider.key_reference, crate::storage::MODEL_KEY_REFERENCE);
        assert_eq!(provider.api_base, "https://api.openai.com/v1");
        assert_eq!(provider.model, "gpt-4o-mini");
    }

    /** 新格式的 JSON 应原样反序列化，不触发迁移逻辑。 */
    #[test]
    fn parses_new_format_without_migration() {
        let settings_json = serde_json::to_string(&UserSettings {
            model_config: test_model_config(),
            privacy_policy: "allow-selected-scope".to_owned(),
            write_confirmation_required: true,
        })
        .unwrap();

        let parsed = parse_or_migrate_user_settings_json(&settings_json, "刚刚").unwrap();

        assert_eq!(parsed.model_config.providers.len(), 2);
        assert_eq!(parsed.model_config.default_provider_id, "provider-a");
    }

    /** 本轮显式 providerId 的优先级必须高于会话默认和全局默认。 */
    #[test]
    fn resolve_provider_prefers_request_over_session_and_default() {
        let model_config = test_model_config();
        let resolved =
            resolve_provider(&model_config, Some("provider-a"), Some("provider-b")).unwrap();

        assert_eq!(resolved.id, "provider-b");
    }

    /** 没有本轮显式选择时，应回退到会话默认。 */
    #[test]
    fn resolve_provider_falls_back_to_session_default() {
        let model_config = test_model_config();
        let resolved = resolve_provider(&model_config, Some("provider-b"), None).unwrap();

        assert_eq!(resolved.id, "provider-b");
    }

    /** 本轮和会话都没有选择时，应使用全局默认 provider。 */
    #[test]
    fn resolve_provider_falls_back_to_global_default() {
        let model_config = test_model_config();
        let resolved = resolve_provider(&model_config, None, None).unwrap();

        assert_eq!(resolved.id, "provider-a");
    }

    /** 找不到对应 providerId 时必须返回可见错误，不能静默切换到其他 provider。 */
    #[test]
    fn resolve_provider_errors_when_provider_missing() {
        let model_config = test_model_config();
        let error = resolve_provider(&model_config, None, Some("missing-provider")).unwrap_err();

        assert!(error.0.contains("missing-provider"));
    }

    /** 已停用的 provider 必须返回可见错误，不能静默切换到其他 provider。 */
    #[test]
    fn resolve_provider_errors_when_provider_disabled() {
        let mut model_config = test_model_config();
        model_config.providers[0].enabled = false;

        let error = resolve_provider(&model_config, None, Some("provider-a")).unwrap_err();

        assert!(error.0.contains("已停用"));
    }

    /** 云端模型整体未启用时必须直接返回错误。 */
    #[test]
    fn resolve_provider_errors_when_model_disabled() {
        let mut model_config = test_model_config();
        model_config.enabled = false;

        let error = resolve_provider(&model_config, None, None).unwrap_err();

        assert!(error.0.contains("未启用"));
    }

    /** endpoint 拼接必须兼容只提供 /v1 base 的情况。 */
    #[test]
    fn chat_completions_endpoint_appends_path_for_v1_base() {
        assert_eq!(
            chat_completions_endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    /** endpoint 拼接必须兼容用户已经填写完整 chat/completions 路径的情况。 */
    #[test]
    fn chat_completions_endpoint_keeps_full_path_untouched() {
        assert_eq!(
            chat_completions_endpoint("https://api.openai.com/v1/chat/completions/"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    /** 迁移出的默认 provider 必须沿用旧 keyReference，避免用户重新配置密钥。 */
    #[test]
    fn key_reference_for_migrated_default_provider_matches_legacy_constant() {
        assert_eq!(
            key_reference_for_provider(MIGRATED_DEFAULT_PROVIDER_ID),
            crate::storage::MODEL_KEY_REFERENCE
        );
    }

    /** 新增 provider 的 key 引用必须按 providerId 隔离，避免互相覆盖。 */
    #[test]
    fn key_reference_for_new_providers_is_isolated_by_id() {
        assert_ne!(
            key_reference_for_provider("provider-a"),
            key_reference_for_provider("provider-b")
        );
    }

    /** 错误脱敏必须移除 Bearer token 和 sk- 密钥片段。 */
    #[test]
    fn redact_model_error_text_removes_bearer_and_secret_key() {
        let redacted = redact_model_error_text(
            r#"HTTP 401 {"error":"invalid api key","header":"Authorization: Bearer sk-test-secret-123"}"#,
        );

        assert!(!redacted.contains("sk-test-secret-123"));
        assert!(!redacted.contains("Bearer sk-test-secret-123"));
        assert!(redacted.contains("[redacted]"));
    }

    /** 错误脱敏必须限制最终文本长度，避免超长响应正文进入可见消息。 */
    #[test]
    fn redact_model_error_text_limits_length() {
        let long_body = "x".repeat(10_000);
        let redacted = redact_model_error_text(&long_body);

        assert!(
            redacted.chars().count() <= MAX_REDACTED_TEXT_CHARS + "…（已截断）".chars().count()
        );
    }

    /** 回归测试：`save_model_api_key` 始终按 providerId 派生 key_reference 写入 keyring，
     * 如果配置里保存的 key_reference 和这个派生值不一致（例如前端新增 provider 时生成的占位引用），
     * 归一化函数必须把它纠正过来，否则运行时用配置里的 key_reference 读密钥会读到别的位置，
     * 复现“已保存密钥但提示未找到”的问题。 */
    #[test]
    fn normalize_model_config_key_references_fixes_mismatched_reference() {
        let mut model_config = test_model_config();

        model_config.providers[0].key_reference = "some-unrelated-placeholder".to_owned();

        normalize_model_config_key_references(&mut model_config);

        assert_eq!(
            model_config.providers[0].key_reference,
            key_reference_for_provider(&model_config.providers[0].id)
        );
    }

    /** 解析新格式设置 JSON 时也要自动修复历史上写入过的不一致 key_reference，无需用户重新配置。 */
    #[test]
    fn parse_or_migrate_user_settings_json_self_heals_mismatched_key_reference() {
        let mut model_config = test_model_config();

        model_config.providers[0].key_reference =
            "stale-reference-from-old-frontend-bug".to_owned();

        let settings_json = serde_json::to_string(&UserSettings {
            model_config,
            privacy_policy: "allow-selected-scope".to_owned(),
            write_confirmation_required: true,
        })
        .unwrap();

        let parsed = parse_or_migrate_user_settings_json(&settings_json, "刚刚").unwrap();

        assert_eq!(
            parsed.model_config.providers[0].key_reference,
            key_reference_for_provider(&parsed.model_config.providers[0].id)
        );
    }

    /** 新格式如果带 providers 字段但 provider 结构损坏，必须报错，不能误迁移成空旧格式丢失多 provider 配置。 */
    #[test]
    fn parse_or_migrate_user_settings_json_rejects_broken_new_format() {
        let broken_new_format_json = r#"{
            "modelConfig": {
                "enabled": true,
                "defaultProviderId": "provider-a",
                "providers": [
                    {
                        "id": "provider-a",
                        "name": "Provider A",
                        "provider": "openai-compatible",
                        "apiBase": "https://llm.example/v1",
                        "model": "test-model",
                        "keyReference": "cici-note-llm-provider-provider-a-api-key",
                        "enabled": true,
                        "supportsTools": true,
                        "requiresApiKey": true,
                        "createdAt": "刚刚"
                    }
                ]
            },
            "privacyPolicy": "allow-selected-scope",
            "writeConfirmationRequired": true
        }"#;

        let error =
            parse_or_migrate_user_settings_json(broken_new_format_json, "刚刚").unwrap_err();

        assert!(error.contains("updatedAt"));
    }
}
