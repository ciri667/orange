use serde::{Deserialize, Serialize};

/** IM 会话的可展示身份；只保留脱敏后的通道指纹，不保存外部平台原始 ID。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImSessionIdentity {
    pub provider_id: String,
    pub conversation_kind: String,
    pub channel_hash: String,
    pub initial_message_preview: String,
    pub last_message_preview: String,
}

/** 首个内置 IM provider ID；后续 provider 继续使用稳定小写 ID。 */
pub const IM_PROVIDER_FEISHU: &str = "feishu";

/** 即时通讯集成总设置；providers 是持久化扩展点，避免新增 IM 时继续扩根字段。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImIntegrationSettings {
    #[serde(default)]
    pub providers: Vec<ImProviderSettings>,
}

/** 单个 IM provider 的通用配置；平台专属字段放在 config 中。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImProviderSettings {
    pub provider_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub default_knowledge_base_ids: Vec<String>,
    #[serde(default)]
    pub allowed_user_open_ids: Vec<String>,
    #[serde(default)]
    pub allowed_chat_ids: Vec<String>,
    #[serde(default)]
    pub discovered_user_open_ids: Vec<String>,
    #[serde(default)]
    pub discovered_chat_ids: Vec<String>,
    pub require_mention: bool,
    pub updated_at: String,
    pub config: ImProviderConfig,
}

/** IM provider 平台专属配置；新增 IM 时在这里增加新变体。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ImProviderConfig {
    #[serde(rename = "feishu")]
    Feishu(FeishuProviderConfig),
}

/** 飞书/Lark 自建应用专属配置；appSecret 单独存 keyring，这里只保存引用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuProviderConfig {
    pub domain: String,
    pub app_id: String,
    pub secret_key_reference: String,
}

/** 飞书/Lark 运行时扁平配置；用于复用首版已有处理逻辑，不作为新持久化结构。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuIntegrationSettings {
    pub enabled: bool,
    pub domain: String,
    pub app_id: String,
    pub secret_key_reference: String,
    #[serde(default)]
    pub default_knowledge_base_ids: Vec<String>,
    #[serde(default)]
    pub allowed_user_open_ids: Vec<String>,
    #[serde(default)]
    pub allowed_chat_ids: Vec<String>,
    #[serde(default)]
    pub discovered_user_open_ids: Vec<String>,
    #[serde(default)]
    pub discovered_chat_ids: Vec<String>,
    pub require_mention: bool,
    pub updated_at: String,
}

impl ImProviderSettings {
    /** 从旧版飞书配置生成 provider 配置，用于 SQLite 旧 JSON 迁移和默认值构造。 */
    pub fn from_feishu(settings: FeishuIntegrationSettings) -> Self {
        Self {
            provider_id: IM_PROVIDER_FEISHU.to_owned(),
            enabled: settings.enabled,
            default_knowledge_base_ids: settings.default_knowledge_base_ids,
            allowed_user_open_ids: settings.allowed_user_open_ids,
            allowed_chat_ids: settings.allowed_chat_ids,
            discovered_user_open_ids: settings.discovered_user_open_ids,
            discovered_chat_ids: settings.discovered_chat_ids,
            require_mention: settings.require_mention,
            updated_at: settings.updated_at,
            config: ImProviderConfig::Feishu(FeishuProviderConfig {
                domain: settings.domain,
                app_id: settings.app_id,
                secret_key_reference: settings.secret_key_reference,
            }),
        }
    }

    /** 将 provider 配置转成飞书运行时配置；非飞书 provider 返回 None。 */
    pub fn to_feishu_settings(&self) -> Option<FeishuIntegrationSettings> {
        match &self.config {
            ImProviderConfig::Feishu(config) if self.provider_id == IM_PROVIDER_FEISHU => {
                Some(FeishuIntegrationSettings {
                    enabled: self.enabled,
                    domain: config.domain.clone(),
                    app_id: config.app_id.clone(),
                    secret_key_reference: config.secret_key_reference.clone(),
                    default_knowledge_base_ids: self.default_knowledge_base_ids.clone(),
                    allowed_user_open_ids: self.allowed_user_open_ids.clone(),
                    allowed_chat_ids: self.allowed_chat_ids.clone(),
                    discovered_user_open_ids: self.discovered_user_open_ids.clone(),
                    discovered_chat_ids: self.discovered_chat_ids.clone(),
                    require_mention: self.require_mention,
                    updated_at: self.updated_at.clone(),
                })
            }
            _ => None,
        }
    }
}

/** IM provider 凭证保存状态；只暴露是否存在，不返回明文。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImProviderCredentialStatus {
    pub provider_id: String,
    pub key_reference: String,
    pub configured: bool,
    pub message: String,
}

/** 兼容旧命令签名的飞书凭证状态别名。 */
pub type FeishuCredentialStatus = ImProviderCredentialStatus;

/** IM provider 长连接网关运行态，设置页用它展示手动启停结果。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImGatewayStatus {
    pub provider_id: String,
    pub running: bool,
    pub connected: bool,
    pub domain: String,
    pub app_id_configured: bool,
    pub secret_configured: bool,
    pub last_started_at: Option<String>,
    pub last_stopped_at: Option<String>,
    pub last_error: Option<String>,
}

/** 兼容旧命令签名的飞书网关状态别名。 */
pub type FeishuGatewayStatus = ImGatewayStatus;
