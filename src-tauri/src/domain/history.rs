use serde::{Deserialize, Serialize};

/** 文档历史记录摘要；正文快照只在读取详情时从 app data 快照文件加载。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHistoryEntry {
    pub id: String,
    pub target_kind: String,
    pub knowledge_base_id: String,
    pub target_id: String,
    pub relative_path: String,
    pub title: String,
    pub file_type: String,
    pub content_hash: String,
    pub byte_size: usize,
    pub line_count: usize,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub created_at: String,
}

/** 文档历史记录详情；content 只在用户打开某一条版本时跨 IPC 传输。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHistoryEntryDetail {
    #[serde(flatten)]
    pub entry: DocumentHistoryEntry,
    pub content: String,
}
