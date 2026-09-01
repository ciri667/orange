use super::agent::AgentSecuritySettings;
use serde::{Deserialize, Serialize};

/** 用户选择的隐私策略，决定模型请求是否允许携带本地笔记片段。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivacyPolicy {
    LocalOnly,
    AllowSelectedScope,
}

/** 默认要求配置 API key；只有本地免鉴权服务（例如 Ollama）会显式关闭。 */
fn default_requires_api_key() -> bool {
    true
}

/** 单个 provider 下可被用户启用和选择的模型条目。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderModel {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    pub enabled: bool,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    pub updated_at: String,
}

/** 单个 LLM Provider 实例配置；用户可以配置多个 provider 并按需切换。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderConfig {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub api_base: String,
    pub model: String,
    pub key_reference: String,
    pub enabled: bool,
    pub supports_tools: bool,
    /** 是否需要配置 API key；本地免鉴权服务可以标记为 false 跳过 key 校验。 */
    #[serde(default = "default_requires_api_key")]
    pub requires_api_key: bool,
    /** 自动发现或手动保留的模型列表；为空时继续使用 model 字段兼容旧配置。 */
    #[serde(default)]
    pub models: Vec<LlmProviderModel>,
    /** 最近一次从 provider API 获取模型列表的本地时间。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_fetched_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/** 云端模型设置聚合多个 Provider；默认 Provider 决定未显式选择时使用哪一个。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub enabled: bool,
    pub default_provider_id: String,
    pub providers: Vec<LlmProviderConfig>,
}

/** 用户设置聚合模型、隐私和写入确认策略，供 M3 Runtime 读取。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    pub model_config: ModelConfig,
    pub privacy_policy: String,
    pub write_confirmation_required: bool,
    #[serde(default)]
    pub agent_security: AgentSecuritySettings,
}

/** 模型密钥保存状态，只暴露是否可读取，不返回明文密钥。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelApiKeyStatus {
    pub provider_id: String,
    pub key_reference: String,
    pub configured: bool,
    pub message: String,
}

/** 用户主动查看时返回的模型密钥；不得进入启动状态或审计日志。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedModelApiKey {
    pub provider_id: String,
    pub api_key: String,
}

/** 刷新 provider 模型列表后的摘要；settings 是归一化并持久化后的完整设置。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmProviderModelRefreshResult {
    pub settings: UserSettings,
    pub provider_id: String,
    pub fetched_at: String,
    pub fetched_count: usize,
    pub model_count: usize,
    pub enabled_count: usize,
    pub message: String,
}
