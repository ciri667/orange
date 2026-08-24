use serde::{Deserialize, Serialize};

/** 模型请求和本地工具调用审计摘要，用于解释每轮 Agent 使用了哪些范围。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestAuditLog {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub scope_summary: String,
    pub content_summary: String,
    pub tool_summary: String,
    pub created_at: String,
}

/** 用户可读的应用事件日志，和模型请求审计分开保存，避免运行诊断污染 Agent 边界说明。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEventLog {
    pub id: String,
    pub level: String,
    pub category: String,
    pub event: String,
    pub message: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_base_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
    pub created_at: String,
}

/** 读取应用事件日志的筛选入参；缺省时返回最近日志。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadAppEventLogsPayload {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}
