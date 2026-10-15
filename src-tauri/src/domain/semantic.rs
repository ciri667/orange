use serde::{Deserialize, Serialize};

/** 独立于聊天模型的 embedding 设置。云端授权绑定配置指纹，不随地址变更沿用。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SemanticSearchSettings {
    pub enabled: bool,
    pub provider: String,
    pub api_base: String,
    pub model: String,
    pub requires_api_key: bool,
    pub min_similarity: f32,
    pub cloud_consent_fingerprint: Option<String>,
}

impl Default for SemanticSearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "ollama".into(),
            api_base: "http://127.0.0.1:11434".into(),
            model: String::new(),
            requires_api_key: false,
            min_similarity: 0.35,
            cloud_consent_fingerprint: None,
        }
    }
}

/** 只包含进度和固定错误文案；绝不通过进度事件携带输入文本。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexStatus {
    pub knowledge_base_id: String,
    pub enabled: bool,
    pub paused: bool,
    pub status: String,
    pub indexed_chunks: usize,
    pub pending_chunks: usize,
    pub failed_chunks: usize,
    pub total_chunks: usize,
    pub dimensions: Option<usize>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCredentialStatus {
    pub configured: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticProbeResult {
    pub dimensions: usize,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticIndexAction {
    pub knowledge_base_id: String,
    pub action: String,
    #[serde(default)]
    pub allow_cloud_upload: bool,
}
