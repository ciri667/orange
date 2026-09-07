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

/** QQ 官方机器人 provider ID。 */
pub const IM_PROVIDER_QQ: &str = "qq";

/** 个人微信 iLink / OpenClaw provider ID。 */
pub const IM_PROVIDER_WEIXIN: &str = "weixin";

/** 个人微信默认 API 根地址；用户一般无需修改。 */
pub const WEIXIN_DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";

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
    #[serde(rename = "qq")]
    Qq(QqProviderConfig),
    #[serde(rename = "weixin")]
    Weixin(WeixinProviderConfig),
}

/** 飞书/Lark 自建应用专属配置；appSecret 单独存 keyring，这里只保存引用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuProviderConfig {
    pub domain: String,
    pub app_id: String,
    pub secret_key_reference: String,
}

/** QQ 官方机器人专属配置；AppSecret 单独存 keyring。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QqProviderConfig {
    pub app_id: String,
    pub secret_key_reference: String,
}

/** 个人微信 iLink 专属配置；bot token 单独存 keyring。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinProviderConfig {
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub base_url: String,
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

    /** 读取 QQ 官方机器人配置；非 QQ provider 返回 None。 */
    pub fn to_qq_config(&self) -> Option<&QqProviderConfig> {
        match &self.config {
            ImProviderConfig::Qq(config) if self.provider_id == IM_PROVIDER_QQ => Some(config),
            _ => None,
        }
    }

    /** 读取个人微信配置；非微信 provider 返回 None。 */
    pub fn to_weixin_config(&self) -> Option<&WeixinProviderConfig> {
        match &self.config {
            ImProviderConfig::Weixin(config) if self.provider_id == IM_PROVIDER_WEIXIN => Some(config),
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

/** 个人微信扫码登录状态；qr 图片只短暂存在于设置页会话。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImLoginStatus {
    pub provider_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_image_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub message: String,
}
