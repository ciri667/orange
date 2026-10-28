use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/** 知识库扫描与索引状态，对应前端 KnowledgeBaseStatus。 */
#[allow(dead_code)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KnowledgeBaseStatus {
    Idle,
    Scanning,
    Ready,
    Error,
}

/** 用户选择的本地 Markdown 知识库元信息。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBase {
    pub id: String,
    pub name: String,
    pub path: String,
    pub description: String,
    pub status: String,
    pub note_count: usize,
    #[serde(default)]
    pub document_count: usize,
    pub updated_at: String,
    pub is_default: bool,
    pub semantic_index_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_report: Option<ScanReport>,
}

/** 支持文档类型的扫描计数，默认补齐五类避免旧快照缺字段时报错。 */
pub fn default_scanned_by_type() -> HashMap<String, usize> {
    HashMap::from([
        ("markdown".to_owned(), 0),
        ("txt".to_owned(), 0),
        ("docx".to_owned(), 0),
        ("pdf".to_owned(), 0),
        ("image".to_owned(), 0),
    ])
}

/** 单次知识库扫描报告，用于向前端说明成功、失败和跳过目录。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub scanned_file_count: usize,
    #[serde(default = "default_scanned_by_type")]
    pub scanned_by_type: HashMap<String, usize>,
    pub failed_file_count: usize,
    pub skipped_directories: Vec<String>,
    pub errors: Vec<String>,
    /// 明确不支持、因此未纳入索引的旧格式文件（如 .doc），供用户改存为 .docx。
    #[serde(default)]
    pub unsupported_files: Vec<String>,
}

/** 单篇 Markdown 笔记，真实内容来自本地文件。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub knowledge_base_id: String,
    pub title: String,
    pub path: String,
    pub content: String,
    pub tags: Vec<String>,
    pub updated_at: String,
    pub backlinks: Vec<String>,
    pub content_hash: String,
}

/** 非 Markdown 文档，txt 带正文，docx/pdf/图片只存只读预览所需元数据。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDocument {
    pub id: String,
    pub knowledge_base_id: String,
    pub title: String,
    pub path: String,
    pub file_type: String,
    pub updated_at: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub preview_available: bool,
}

/** docx 只读预览的段落级文本块。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreviewBlock {
    pub r#type: String,
    pub text: String,
    /// 1-based 结构块序号，与 read/search 的 blockIndex 对齐，供引用跳转定位。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_row: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_col: Option<usize>,
}

/** 只读文档抽取的单个结构块；DOCX 使用块序号，PDF 使用页码。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentTextBlock {
    pub index: usize,
    pub r#type: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_row: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table_col: Option<usize>,
}

/** 本地解析后的只读文档正文；不进入 WorkspaceSnapshot，避免把二进制文档正文长期驻留内存。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentTextExtraction {
    pub document_id: String,
    pub file_type: String,
    pub content_hash: String,
    pub blocks: Vec<DocumentTextBlock>,
    pub content_chars: usize,
    pub page_count: usize,
    pub status: String,
    pub warnings: Vec<String>,
}

/** 非 Markdown 文档预览返回值，pdf/图片使用 assetPath，docx 使用 blocks。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPreview {
    pub document_id: String,
    pub file_type: String,
    pub title: String,
    pub path: String,
    pub updated_at: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<DocumentPreviewBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<usize>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 该文档已写入的检索 chunk 数；None 表示没查（例如离线测试）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_chunk_count: Option<usize>,
}

/** 本地知识库中的真实目录，用于让空文件夹也能出现在目录树中。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub id: String,
    pub knowledge_base_id: String,
    pub name: String,
    pub path: String,
    pub updated_at: String,
}

/** Tauri 目录选择器返回的知识库目录信息。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseSelection {
    pub id: String,
    pub name: String,
    pub path: String,
    pub note_count: usize,
}

/** 单张待保存的粘贴图片；bytesBase64 只在命令边界传输，不能写入日志。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteImageAttachmentInput {
    pub mime_type: String,
    pub bytes_base64: String,
    #[serde(default)]
    pub original_file_name: Option<String>,
}

/** 已保存的图片附件，relativePath 是相对当前 Markdown 文件的引用路径。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedNoteImageAttachment {
    pub relative_path: String,
    pub markdown: String,
    pub mime_type: String,
    pub byte_size: usize,
}
