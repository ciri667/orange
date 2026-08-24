use super::*;

pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/** 计算二进制文件 hash，用于 pdf/docx/txt 扫描后识别外部修改。 */
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/** 粘贴图片识别后的安全格式，只允许浏览器和 Markdown 预览能直接显示的常见位图。 */

pub fn create_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/** 生成本地可读日期时间，用于长期展示的会话和审计记录时间。 */
pub(crate) fn format_local_datetime() -> String {
    Local::now().format("%Y/%m/%d %H:%M").to_string()
}

/** 将操作系统文件时间格式化为本地可读日期时间，用于展示真实文件更新时间。 */
pub(crate) fn format_local_datetime_from_system_time(system_time: SystemTime) -> String {
    let datetime: chrono::DateTime<Local> = system_time.into();

    datetime.format("%Y/%m/%d %H:%M").to_string()
}

/** 读取操作系统返回的文件修改时间，失败时返回错误交给调用层记录或汇总。 */
pub(crate) fn file_modified_local_datetime(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("无法读取文件元数据：{error}"))?;
    let modified_at = metadata
        .modified()
        .map_err(|error| format!("无法读取文件修改时间：{error}"))?;

    Ok(format_local_datetime_from_system_time(modified_at))
}

/** 将毫秒时间戳格式化为本地可读日期时间，用于迁移前端旧会话 ID 中的创建时间。 */
pub(crate) fn format_local_datetime_from_millis(timestamp_millis: i64) -> Option<String> {
    Local
        .timestamp_millis_opt(timestamp_millis)
        .single()
        .map(|datetime| datetime.format("%Y/%m/%d %H:%M").to_string())
}

pub fn create_stable_note_id(knowledge_base_id: &str, relative_path: &str) -> String {
    let mut hasher = Sha256::new();

    // 知识库 ID 与路径共同参与 hash，同名文件在不同知识库中不会冲突。
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update(b":");
    hasher.update(relative_path.as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("note-{}", &digest[..24])
}

/** 根据知识库和相对路径生成稳定普通文档 ID，避免与 Markdown note ID 混淆。 */
pub fn create_stable_document_id(knowledge_base_id: &str, relative_path: &str) -> String {
    let mut hasher = Sha256::new();

    // 非 Markdown 文档不进入 Agent note 模型，使用独立前缀可以避免旧会话引用误配。
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update(b":document:");
    hasher.update(relative_path.as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("document-{}", &digest[..24])
}

/** 根据知识库和相对目录路径生成稳定目录 ID，让空目录在重扫后仍能保持稳定节点。 */
pub fn create_stable_folder_id(knowledge_base_id: &str, relative_path: &str) -> String {
    let mut hasher = Sha256::new();

    // 目录 ID 使用独立前缀，避免与同名 Markdown 文件的稳定 ID 混淆。
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update(b":folder:");
    hasher.update(relative_path.as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("folder-{}", &digest[..24])
}
