use super::*;

pub(crate) struct ImageAttachmentFormat {
    mime_type: &'static str,
    extension: &'static str,
}

/** 已完成校验的待写入图片，避免写文件过程中才发现 MIME 或大小不合法。 */
pub(crate) struct PreparedImageAttachment {
    bytes: Vec<u8>,
    format: ImageAttachmentFormat,
    hash_prefix: String,
}

/** 单次写入完成后的文件位置，分别服务清理和 Markdown 插入。 */
pub(crate) struct WrittenImageAttachment {
    absolute_path: PathBuf,
    markdown_relative_path: String,
}

/** 为当前 Markdown 笔记保存粘贴图片附件，返回可插入正文的 Markdown 图片片段。 */
pub fn save_note_image_attachments(
    root: &Path,
    note_relative_path: &str,
    images: &[NoteImageAttachmentInput],
) -> Result<Vec<SavedNoteImageAttachment>, String> {
    if images.is_empty() {
        return Err("没有可保存的图片。".to_owned());
    }

    let note_path = resolve_existing_file_inside_root(root, note_relative_path)?;

    if !is_markdown_file(&note_path) {
        return Err("只能为 Markdown 笔记保存图片附件。".to_owned());
    }

    let prepared_images = prepare_image_attachments(images)?;
    let note_directory = get_relative_parent_path(note_relative_path);
    let attachment_note_folder = attachment_folder_name_from_note_path(note_relative_path);
    let attachment_directory =
        join_relative_path_parts(&[&note_directory, "assets", &attachment_note_folder]);
    let markdown_directory = join_relative_path_parts(&["assets", &attachment_note_folder]);
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut saved_files = Vec::new();
    let mut saved_attachments = Vec::new();

    for prepared_image in prepared_images {
        let file_stem = format!("pasted-{timestamp}-{}", prepared_image.hash_prefix);
        let write_result = write_unique_image_attachment(
            root,
            &attachment_directory,
            &markdown_directory,
            &file_stem,
            prepared_image.format.extension,
            &prepared_image.bytes,
        );

        let written_image = match write_result {
            Ok(written_image) => written_image,
            Err(error) => {
                remove_written_files_best_effort(&saved_files);
                return Err(error);
            }
        };
        let byte_size = prepared_image.bytes.len();
        let markdown_path = encode_markdown_image_path(&written_image.markdown_relative_path);

        saved_files.push(written_image.absolute_path);
        saved_attachments.push(SavedNoteImageAttachment {
            relative_path: written_image.markdown_relative_path,
            markdown: format!("![image]({markdown_path})"),
            mime_type: prepared_image.format.mime_type.to_owned(),
            byte_size,
        });
    }

    Ok(saved_attachments)
}

/** 解码并校验整批图片；任何一张失败都不会进入文件写入阶段。 */
pub(crate) fn prepare_image_attachments(
    images: &[NoteImageAttachmentInput],
) -> Result<Vec<PreparedImageAttachment>, String> {
    let mut total_bytes = 0usize;
    let mut prepared_images = Vec::with_capacity(images.len());

    for image in images {
        // 原始文件名可能包含用户隐私，首版只保留接口兼容性，不参与命名、日志或错误信息。
        let _ignored_original_file_name = image.original_file_name.as_deref();
        let expected_mime_type = normalize_image_mime_type(&image.mime_type)?;
        let bytes = decode_image_base64(&image.bytes_base64)?;

        if bytes.is_empty() {
            return Err("图片内容为空，已阻止保存。".to_owned());
        }

        if bytes.len() > MAX_SINGLE_PASTE_IMAGE_BYTES {
            return Err("单张图片超过 20MB，已阻止保存。".to_owned());
        }

        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| "图片总大小超过限制，已阻止保存。".to_owned())?;

        if total_bytes > MAX_PASTE_IMAGE_BATCH_BYTES {
            return Err("单次粘贴图片总大小超过 50MB，已阻止保存。".to_owned());
        }

        let detected_format = detect_image_attachment_format(&bytes)?;

        if expected_mime_type != detected_format.mime_type {
            return Err("图片 MIME 类型与文件内容不一致，已阻止保存。".to_owned());
        }

        let digest = hash_bytes(&bytes);
        let hash_prefix = digest
            .chars()
            .take(PASTED_IMAGE_HASH_PREFIX_LENGTH)
            .collect::<String>();

        prepared_images.push(PreparedImageAttachment {
            bytes,
            format: detected_format,
            hash_prefix,
        });
    }

    Ok(prepared_images)
}

/** 标准化剪贴板 MIME；image/jpg 作为常见别名接受，但仍按内容校验为 JPEG。 */
pub(crate) fn normalize_image_mime_type(mime_type: &str) -> Result<&'static str, String> {
    let normalized_mime_type = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    match normalized_mime_type.as_str() {
        "image/png" => Ok("image/png"),
        "image/jpeg" | "image/jpg" => Ok("image/jpeg"),
        "image/webp" => Ok("image/webp"),
        "image/gif" => Ok("image/gif"),
        _ => Err("仅支持 png、jpeg、webp 和 gif 图片。".to_owned()),
    }
}

/** 从前端传入的 base64 或 data URL 中提取图片字节，不把原文写入错误信息。 */
pub(crate) fn decode_image_base64(bytes_base64: &str) -> Result<Vec<u8>, String> {
    let encoded_body = bytes_base64
        .split_once(',')
        .map(|(_, body)| body)
        .unwrap_or(bytes_base64)
        .trim();

    BASE64_STANDARD
        .decode(encoded_body)
        .map_err(|_| "图片内容不是有效的 base64，已阻止保存。".to_owned())
}

/** 根据文件头识别图片真实格式，防止伪造 MIME 的 SVG 或其他文件落盘。 */
pub(crate) fn detect_image_attachment_format(
    bytes: &[u8],
) -> Result<ImageAttachmentFormat, String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Ok(ImageAttachmentFormat {
            mime_type: "image/png",
            extension: "png",
        });
    }

    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Ok(ImageAttachmentFormat {
            mime_type: "image/jpeg",
            extension: "jpg",
        });
    }

    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(ImageAttachmentFormat {
            mime_type: "image/gif",
            extension: "gif",
        });
    }

    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Ok(ImageAttachmentFormat {
            mime_type: "image/webp",
            extension: "webp",
        });
    }

    Err("无法识别图片格式，已阻止保存。".to_owned())
}

pub fn should_walk_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }

    let Some(name) = entry.file_name().to_str() else {
        return true;
    };

    // 隐藏目录和常见构建产物通常不是用户知识内容，跳过可以明显降低误选大目录时的卡顿。
    !name.starts_with('.') && !IGNORED_DIRECTORY_NAMES.contains(&name)
}

pub fn extract_markdown_title(path: &Path, content: &str) -> String {
    content
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("# ")
                .map(str::trim)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名笔记")
                .to_owned()
        })
}

/** 本地扫描支持的文件类型，Markdown 进入 Agent note，其余进入普通文档模型。 */
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupportedDocumentKind {
    Markdown,
    Txt,
    Docx,
    Pdf,
    Image,
}

impl SupportedDocumentKind {
    /** 返回扫描报告中的稳定类型 key，对应前端 scannedByType。 */
    fn scan_key(self) -> &'static str {
        match self {
            SupportedDocumentKind::Markdown => "markdown",
            SupportedDocumentKind::Txt => "txt",
            SupportedDocumentKind::Docx => "docx",
            SupportedDocumentKind::Pdf => "pdf",
            SupportedDocumentKind::Image => "image",
        }
    }

    /** 返回前端 WorkspaceDocument.fileType 使用的非 Markdown 类型。 */
    fn document_file_type(self) -> Option<&'static str> {
        match self {
            SupportedDocumentKind::Markdown => None,
            SupportedDocumentKind::Txt => Some("txt"),
            SupportedDocumentKind::Docx => Some("docx"),
            SupportedDocumentKind::Pdf => Some("pdf"),
            SupportedDocumentKind::Image => Some("image"),
        }
    }

    /** 返回用户可读类型名称，用于扫描错误提示。 */
    fn label(self) -> &'static str {
        match self {
            SupportedDocumentKind::Markdown => "Markdown",
            SupportedDocumentKind::Txt => "TXT",
            SupportedDocumentKind::Docx => "DOCX",
            SupportedDocumentKind::Pdf => "PDF",
            SupportedDocumentKind::Image => "图片",
        }
    }
}

/** 根据扩展名识别首版支持的文档类型。 */
pub(crate) fn supported_document_kind(path: &Path) -> Option<SupportedDocumentKind> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md") | Some("markdown") => Some(SupportedDocumentKind::Markdown),
        Some("txt") => Some(SupportedDocumentKind::Txt),
        Some("docx") => Some(SupportedDocumentKind::Docx),
        Some("pdf") => Some(SupportedDocumentKind::Pdf),
        Some("png") | Some("jpg") | Some("jpeg") | Some("webp") | Some("gif") | Some("svg") => {
            Some(SupportedDocumentKind::Image)
        }
        _ => None,
    }
}

/** 从文件名提取普通文档标题，避免 docx/pdf/图片预览前还要读取二进制正文。 */
pub(crate) fn document_title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("未命名文档")
        .to_owned()
}

/** 生成扫描报告的类型计数容器，所有支持类型都预置为 0。 */
pub(crate) fn create_scanned_by_type_counter() -> HashMap<String, usize> {
    crate::domain::default_scanned_by_type()
}

/** 记录一次成功扫描的支持文件类型。 */
pub(crate) fn increment_scanned_by_type(
    scanned_by_type: &mut HashMap<String, usize>,
    kind: SupportedDocumentKind,
) {
    *scanned_by_type
        .entry(kind.scan_key().to_owned())
        .or_insert(0) += 1;
}

/** 扫描用户选择的支持文档目录，并生成知识库、目录、Markdown 笔记与普通文档快照。 */
pub fn scan_supported_documents_directory(
    selection: &KnowledgeBaseSelection,
) -> Result<
    (
        KnowledgeBase,
        Vec<FolderEntry>,
        Vec<Note>,
        Vec<WorkspaceDocument>,
    ),
    String,
> {
    let root = fs::canonicalize(&selection.path)
        .map_err(|error| format!("无法访问知识库目录：{error}"))?;
    let mut folders = Vec::new();
    let mut notes = Vec::new();
    let mut documents = Vec::new();
    let mut errors = Vec::new();
    let mut scanned_by_type = create_scanned_by_type_counter();
    let mut skipped_directory_set = HashSet::new();
    let root_for_filter = root.clone();

    for entry in WalkDir::new(&root).into_iter().filter_entry(|entry| {
        let should_walk = should_walk_entry(entry);

        if !should_walk {
            // 被跳过的目录写入扫描报告，帮助用户理解项目根目录为何只索引 Markdown 内容区。
            let skipped_path = entry
                .path()
                .strip_prefix(&root_for_filter)
                .unwrap_or_else(|_| entry.path())
                .to_string_lossy()
                .replace('\\', "/");

            skipped_directory_set.insert(skipped_path);
        }

        should_walk
    }) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("遍历目录失败：{error}"));
                continue;
            }
        };
        let path = entry.path();

        if path.is_dir() && entry.depth() > 0 {
            let relative_path = path
                .strip_prefix(&root)
                .map_err(|error| format!("无法计算目录相对路径：{error}"))?
                .to_string_lossy()
                .replace('\\', "/");
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名目录")
                .to_owned();

            folders.push(FolderEntry {
                id: create_stable_folder_id(&selection.id, &relative_path),
                knowledge_base_id: selection.id.clone(),
                name,
                path: relative_path,
                updated_at: "刚刚".to_owned(),
            });
            continue;
        }

        let Some(document_kind) = path
            .is_file()
            .then(|| supported_document_kind(path))
            .flatten()
        else {
            continue;
        };
        let relative_path = path
            .strip_prefix(&root)
            .map_err(|error| format!("无法计算相对路径：{error}"))?
            .to_string_lossy()
            .replace('\\', "/");

        let updated_at = match file_modified_local_datetime(path) {
            Ok(updated_at) => updated_at,
            Err(error) => {
                errors.push(format!("无法读取文件更新时间 {}：{error}", path.display()));
                format_local_datetime()
            }
        };

        match document_kind {
            SupportedDocumentKind::Markdown => {
                let content = match fs::read_to_string(path) {
                    Ok(content) => content,
                    Err(error) => {
                        errors.push(format!(
                            "无法读取 Markdown 文件 {}：{error}",
                            path.display()
                        ));
                        continue;
                    }
                };
                let title = extract_markdown_title(path, &content);
                let tags = extract_tags(&content);

                notes.push(Note {
                    id: create_stable_note_id(&selection.id, &relative_path),
                    knowledge_base_id: selection.id.clone(),
                    title,
                    path: relative_path,
                    content_hash: hash_content(&content),
                    content,
                    tags,
                    updated_at,
                    backlinks: Vec::new(),
                });
            }
            SupportedDocumentKind::Txt => {
                let content = match fs::read_to_string(path) {
                    Ok(content) => content,
                    Err(error) => {
                        errors.push(format!("无法读取 TXT 文件 {}：{error}", path.display()));
                        continue;
                    }
                };

                documents.push(WorkspaceDocument {
                    id: create_stable_document_id(&selection.id, &relative_path),
                    knowledge_base_id: selection.id.clone(),
                    title: document_title_from_path(path),
                    path: relative_path,
                    file_type: "txt".to_owned(),
                    updated_at,
                    content_hash: hash_content(&content),
                    content: Some(content),
                    preview_available: false,
                });
            }
            SupportedDocumentKind::Docx
            | SupportedDocumentKind::Pdf
            | SupportedDocumentKind::Image => {
                let bytes = match fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        errors.push(format!(
                            "无法读取 {} 文件 {}：{error}",
                            document_kind.label(),
                            path.display()
                        ));
                        continue;
                    }
                };
                let file_type = document_kind
                    .document_file_type()
                    .expect("非 Markdown 文档必须有 fileType");

                documents.push(WorkspaceDocument {
                    id: create_stable_document_id(&selection.id, &relative_path),
                    knowledge_base_id: selection.id.clone(),
                    title: document_title_from_path(path),
                    path: relative_path,
                    file_type: file_type.to_owned(),
                    updated_at,
                    content_hash: hash_bytes(&bytes),
                    content: None,
                    preview_available: true,
                });
            }
        }

        increment_scanned_by_type(&mut scanned_by_type, document_kind);
    }

    folders.sort_by(|left, right| left.path.cmp(&right.path));
    notes.sort_by(|left, right| left.path.cmp(&right.path));
    documents.sort_by(|left, right| left.path.cmp(&right.path));

    let mut skipped_directories: Vec<String> = skipped_directory_set.into_iter().collect();
    skipped_directories.sort();
    let scanned_file_count = notes.len() + documents.len();
    let scan_report = ScanReport {
        scanned_file_count,
        scanned_by_type,
        failed_file_count: errors.len(),
        skipped_directories,
        errors,
    };

    let knowledge_base = KnowledgeBase {
        id: selection.id.clone(),
        name: selection.name.clone(),
        path: root.to_string_lossy().to_string(),
        description: if scan_report.failed_file_count > 0 {
            format!(
                "已扫描 {} 个支持文档，{} 个文件失败。",
                scan_report.scanned_file_count, scan_report.failed_file_count
            )
        } else {
            "通过 Tauri 扫描的本地支持文档知识库。".to_owned()
        },
        status: "ready".to_owned(),
        note_count: notes.len(),
        document_count: documents.len(),
        updated_at: "刚刚".to_owned(),
        is_default: false,
        semantic_index_enabled: false,
        scan_report: Some(scan_report),
    };

    Ok((knowledge_base, folders, notes, documents))
}

/** 兼容旧调用方的 Markdown-only 扫描包装，普通文档会被扫描报告统计但不返回。 */
#[allow(dead_code)]
pub fn scan_markdown_directory(
    selection: &KnowledgeBaseSelection,
) -> Result<(KnowledgeBase, Vec<FolderEntry>, Vec<Note>), String> {
    let (knowledge_base, folders, notes, _documents) =
        scan_supported_documents_directory(selection)?;

    Ok((knowledge_base, folders, notes))
}

/** 判断路径是否为 Markdown 文件。 */
pub fn is_markdown_file(path: &Path) -> bool {
    supported_document_kind(path) == Some(SupportedDocumentKind::Markdown)
}

/** 判断路径是否为可编辑 txt 文档。 */
pub fn is_text_document_file(path: &Path) -> bool {
    supported_document_kind(path) == Some(SupportedDocumentKind::Txt)
}

/** 判断路径是否为只读预览 pdf 文档。 */
pub(crate) fn is_pdf_document_file(path: &Path) -> bool {
    supported_document_kind(path) == Some(SupportedDocumentKind::Pdf)
}

/** 判断路径是否为只读预览 docx 文档。 */
pub(crate) fn is_docx_document_file(path: &Path) -> bool {
    supported_document_kind(path) == Some(SupportedDocumentKind::Docx)
}

/** 判断路径是否为只读预览图片文档。 */
pub(crate) fn is_image_document_file(path: &Path) -> bool {
    supported_document_kind(path) == Some(SupportedDocumentKind::Image)
}

/** 校验用户输入的新文件名，只允许当前目录下的 Markdown 文件名。 */
pub fn validate_markdown_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let requested_path = Path::new(trimmed_file_name);

    // 重命名只允许改当前文件名，不能携带路径分隔符或特殊路径组件。
    if requested_path.components().count() != 1
        || trimmed_file_name.contains('/')
        || trimmed_file_name.contains('\\')
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件名不能包含路径或上级目录。".to_owned());
    }

    let extension = requested_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if !matches!(extension.as_deref(), Some("md") | Some("markdown")) {
        return Err("文件名必须以 .md 或 .markdown 结尾。".to_owned());
    }

    Ok(trimmed_file_name.to_owned())
}

/** 校验新建 Markdown 文件名；允许省略扩展名，省略时默认补 .md。 */
pub fn validate_new_markdown_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let normalized_file_name = if Path::new(trimmed_file_name).extension().is_none() {
        format!("{trimmed_file_name}.md")
    } else {
        trimmed_file_name.to_owned()
    };

    validate_markdown_file_name(&normalized_file_name)
}

/** 校验用户输入的新文件名，只允许当前目录下的 txt 文件名。 */
pub fn validate_text_document_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let requested_path = Path::new(trimmed_file_name);

    // txt 重命名同样只允许改当前文件名，防止把普通文档操作变成移动或越权写入。
    if requested_path.components().count() != 1
        || trimmed_file_name.contains('/')
        || trimmed_file_name.contains('\\')
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件名不能包含路径或上级目录。".to_owned());
    }

    let extension = requested_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if !matches!(extension.as_deref(), Some("txt")) {
        return Err("文件名必须以 .txt 结尾。".to_owned());
    }

    Ok(trimmed_file_name.to_owned())
}

/** 校验新建 txt 文件名；允许省略扩展名，省略时默认补 .txt。 */
pub fn validate_new_text_document_file_name(file_name: &str) -> Result<String, String> {
    let trimmed_file_name = file_name.trim();

    if trimmed_file_name.is_empty() {
        return Err("文件名不能为空。".to_owned());
    }

    let normalized_file_name = if Path::new(trimmed_file_name).extension().is_none() {
        format!("{trimmed_file_name}.txt")
    } else {
        trimmed_file_name.to_owned()
    };

    validate_text_document_file_name(&normalized_file_name)
}

/** 校验新建文件夹名，只允许单级普通目录名，并拒绝扫描忽略目录。 */
pub fn validate_folder_name(folder_name: &str) -> Result<String, String> {
    let trimmed_folder_name = folder_name.trim();

    if trimmed_folder_name.is_empty() {
        return Err("文件夹名不能为空。".to_owned());
    }

    let requested_path = Path::new(trimmed_folder_name);

    // 新建目录只允许单级名称，不能通过分隔符或特殊组件创建多级/越界路径。
    if requested_path.components().count() != 1
        || trimmed_folder_name.contains('/')
        || trimmed_folder_name.contains('\\')
        || matches!(trimmed_folder_name, "." | "..")
        || requested_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件夹名不能包含路径或上级目录。".to_owned());
    }

    if trimmed_folder_name.starts_with('.')
        || IGNORED_DIRECTORY_NAMES.contains(&trimmed_folder_name)
    {
        return Err("不能创建隐藏目录或扫描忽略目录。".to_owned());
    }

    Ok(trimmed_folder_name.to_owned())
}

/** 校验目标文件必须位于知识库根目录内，防止路径穿越或越权写入。 */
pub fn resolve_inside_root(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let requested_path = Path::new(relative_path);

    // 先做纯路径组件检查，再创建父目录，避免路径穿越时在知识库外生成目录。
    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let joined_path = canonical_root.join(requested_path);
    let parent = joined_path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;

    fs::create_dir_all(parent).map_err(|error| format!("无法创建目标父目录：{error}"))?;
    let canonical_parent =
        fs::canonicalize(parent).map_err(|error| format!("无法解析目标父目录：{error}"))?;

    // canonicalize 目标文件本身在新建文件时会失败，所以只校验父目录边界。
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    Ok(joined_path)
}

/** 校验已存在文件位于知识库根目录内，保存已有笔记时不创建任何新目录。 */
pub fn resolve_existing_file_inside_root(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, String> {
    let requested_path = Path::new(relative_path);

    // 保存已有笔记只接受普通相对路径，防止前端快照被篡改后指向根目录外文件。
    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let canonical_target = fs::canonicalize(canonical_root.join(requested_path))
        .map_err(|error| format!("无法解析目标文件：{error}"))?;

    // canonicalize 目标文件可以拦截指向根目录外的符号链接，确保保存不会越权。
    if !canonical_target.starts_with(&canonical_root) || !canonical_target.is_file() {
        return Err("目标路径超出知识库根目录，已阻止写入".to_owned());
    }

    Ok(canonical_target)
}

/** 校验父目录必须是知识库内已经存在的目录，避免新建操作隐式创建多级路径。 */
pub fn resolve_existing_directory_inside_root(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, String> {
    let trimmed_relative_path = relative_path.trim().trim_matches('/');
    let requested_path = Path::new(trimmed_relative_path);

    if requested_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("目标目录超出知识库根目录，已阻止新建。".to_owned());
    }

    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let target_path = if trimmed_relative_path.is_empty() {
        canonical_root.clone()
    } else {
        fs::canonicalize(canonical_root.join(requested_path))
            .map_err(|error| format!("无法解析目标目录：{error}"))?
    };

    // canonicalize 可以拦截指向根目录外的符号链接，确保新建不会越权。
    if !target_path.starts_with(&canonical_root) || !target_path.is_dir() {
        return Err("目标目录超出知识库根目录，已阻止新建。".to_owned());
    }

    Ok(target_path)
}

/** 获取知识库内相对文件路径的父目录，返回值始终使用 / 作为分隔符。 */
pub(crate) fn get_relative_parent_path(relative_path: &str) -> String {
    relative_path
        .trim()
        .trim_matches('/')
        .rsplit_once('/')
        .map(|(parent_path, _file_name)| parent_path.to_owned())
        .unwrap_or_default()
}

/** 从 Markdown 文件名生成附件子目录名，避免隐藏目录、路径字符和控制字符进入本地路径。 */
pub(crate) fn attachment_folder_name_from_note_path(note_relative_path: &str) -> String {
    let file_name = note_relative_path
        .trim()
        .trim_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let file_stem = file_name
        .rsplit_once('.')
        .map(|(stem, _extension)| stem)
        .unwrap_or(file_name);
    let sanitized_name = file_stem
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            character if character.is_control() => '-',
            character => character,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_owned();

    if sanitized_name.is_empty() || sanitized_name.starts_with('.') {
        DEFAULT_ATTACHMENT_NOTE_FOLDER_NAME.to_owned()
    } else {
        sanitized_name
    }
}

/** 拼接知识库内相对路径片段，过滤空片段并统一使用 /，避免平台分隔符进入 Markdown。 */
pub(crate) fn join_relative_path_parts(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim().trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/** 以不覆盖方式写入图片附件，遇到同秒同 hash 重名时追加序号继续尝试。 */
pub(crate) fn write_unique_image_attachment(
    root: &Path,
    attachment_directory: &str,
    markdown_directory: &str,
    file_stem: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<WrittenImageAttachment, String> {
    for duplicate_index in 0..1_000 {
        let file_name = if duplicate_index == 0 {
            format!("{file_stem}.{extension}")
        } else {
            format!("{file_stem}-{}.{extension}", duplicate_index + 1)
        };
        let knowledge_base_relative_path =
            join_relative_path_parts(&[attachment_directory, &file_name]);
        let markdown_relative_path = join_relative_path_parts(&[markdown_directory, &file_name]);
        let target_path = resolve_inside_root(root, &knowledge_base_relative_path)?;
        let mut target_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target_path)
        {
            Ok(target_file) => target_file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("无法创建图片附件：{error}")),
        };

        if let Err(error) = target_file.write_all(bytes) {
            let _ = fs::remove_file(&target_path);
            return Err(format!("无法写入图片附件：{error}"));
        }

        return Ok(WrittenImageAttachment {
            absolute_path: target_path,
            markdown_relative_path,
        });
    }

    Err("无法生成可用的图片附件文件名，请稍后重试。".to_owned())
}

/** 对 Markdown 图片路径做最小 URL 转义，保证空格、中文和括号都能被标准 Markdown 解析。 */
pub(crate) fn encode_markdown_image_path(path: &str) -> String {
    let mut encoded_path = String::new();

    for byte in path.as_bytes() {
        let character = *byte as char;

        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~' | '/') {
            encoded_path.push(character);
        } else {
            encoded_path.push_str(&format!("%{byte:02X}"));
        }
    }

    encoded_path
}

/** 批量写入后续失败时清理本次已经创建的文件；清理失败不覆盖原始业务错误。 */
pub(crate) fn remove_written_files_best_effort(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

/** 在指定目录创建不覆盖已有文件的空白 Markdown，并返回相对路径。 */
pub fn create_blank_markdown_file(
    root: &Path,
    parent_relative_path: &str,
    requested_file_name: Option<&str>,
) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let parent_path =
        resolve_existing_directory_inside_root(&canonical_root, parent_relative_path)?;
    let file_name = match requested_file_name {
        Some(file_name) => validate_new_markdown_file_name(file_name)?,
        None => next_available_markdown_file_name(&parent_path)?,
    };
    let target_path = parent_path.join(file_name);

    if target_path.exists() {
        return Err("目标文件已存在，已阻止覆盖。".to_owned());
    }

    atomic_write_markdown(&target_path, "")?;

    let canonical_target =
        fs::canonicalize(&target_path).map_err(|error| format!("无法解析新建文件：{error}"))?;

    canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算新建文件相对路径：{error}"))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/** 在指定目录创建不覆盖已有文件的空白 txt 文档，并返回相对路径。 */
pub fn create_blank_text_document_file(
    root: &Path,
    parent_relative_path: &str,
    requested_file_name: Option<&str>,
) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let parent_path =
        resolve_existing_directory_inside_root(&canonical_root, parent_relative_path)?;
    let file_name = match requested_file_name {
        Some(file_name) => validate_new_text_document_file_name(file_name)?,
        None => next_available_text_document_file_name(&parent_path)?,
    };
    let target_path = parent_path.join(file_name);

    if target_path.exists() {
        return Err("目标文件已存在，已阻止覆盖。".to_owned());
    }

    atomic_write_text_document(&target_path, "")?;

    let canonical_target =
        fs::canonicalize(&target_path).map_err(|error| format!("无法解析新建文件：{error}"))?;

    canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算新建文件相对路径：{error}"))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/** 在指定目录创建单级文件夹，并返回相对路径。 */
pub fn create_folder(
    root: &Path,
    parent_relative_path: &str,
    requested_folder_name: &str,
) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let parent_path =
        resolve_existing_directory_inside_root(&canonical_root, parent_relative_path)?;
    let folder_name = validate_folder_name(requested_folder_name)?;
    let target_path = parent_path.join(folder_name);

    if target_path.exists() {
        return Err("目标文件夹已存在，已阻止覆盖。".to_owned());
    }

    fs::create_dir(&target_path).map_err(|error| format!("无法创建文件夹：{error}"))?;

    let canonical_target =
        fs::canonicalize(&target_path).map_err(|error| format!("无法解析新建文件夹：{error}"))?;

    canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算新建文件夹相对路径：{error}"))
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/** 生成指定目录下可用的默认 Markdown 文件名。 */
pub(crate) fn next_available_markdown_file_name(parent_path: &Path) -> Result<String, String> {
    for index in 1..=999 {
        let file_name = if index == 1 {
            "未命名.md".to_owned()
        } else {
            format!("未命名 {index}.md")
        };

        // 用户主动新建笔记不能覆盖已有 Markdown，遇到重名就继续寻找下一个可用文件名。
        if parent_path.join(&file_name).exists() {
            continue;
        }

        return Ok(file_name);
    }

    Err("无法生成未命名笔记路径，请清理过多未命名文件后重试。".to_owned())
}

/** 生成指定目录下可用的默认 txt 文件名。 */
pub(crate) fn next_available_text_document_file_name(parent_path: &Path) -> Result<String, String> {
    for index in 1..=999 {
        let file_name = if index == 1 {
            "未命名.txt".to_owned()
        } else {
            format!("未命名 {index}.txt")
        };

        // txt 新建不能覆盖任何同名文件，包含 docx/pdf/Markdown 等其他类型。
        if parent_path.join(&file_name).exists() {
            continue;
        }

        return Ok(file_name);
    }

    Err("无法生成未命名 TXT 路径，请清理过多未命名文件后重试。".to_owned())
}

/** 重命名已有 Markdown 文件，只修改文件名并返回新相对路径、当前正文和 hash。 */
pub fn rename_markdown_file(
    root: &Path,
    current_relative_path: &str,
    next_file_name: &str,
) -> Result<(String, String, String), String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let current_path = resolve_existing_file_inside_root(&canonical_root, current_relative_path)?;

    if !is_markdown_file(&current_path) {
        return Err("只能重命名 Markdown 文件。".to_owned());
    }

    let safe_file_name = validate_markdown_file_name(next_file_name)?;
    let target_path = current_path.with_file_name(safe_file_name);
    let target_parent = target_path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let canonical_target_parent =
        fs::canonicalize(target_parent).map_err(|error| format!("无法解析目标父目录：{error}"))?;

    // 目标父目录必须仍在知识库内，防止通过异常路径或符号链接逃逸。
    if !canonical_target_parent.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止重命名。".to_owned());
    }

    if target_path.exists() {
        return Err("目标文件名已存在，已阻止覆盖。".to_owned());
    }

    fs::rename(&current_path, &target_path)
        .map_err(|error| format!("无法重命名 Markdown 文件：{error}"))?;

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取重命名后的 Markdown 文件：{error}"))?;
    let canonical_target = fs::canonicalize(&target_path)
        .map_err(|error| format!("无法解析重命名后的文件：{error}"))?;
    let next_relative_path = canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算重命名后的相对路径：{error}"))?
        .to_string_lossy()
        .replace('\\', "/");
    let current_hash = hash_content(&current_content);

    Ok((next_relative_path, current_content, current_hash))
}

/** 重命名已有 txt 文档，只修改文件名并返回新相对路径、当前正文和 hash。 */
pub fn rename_text_document_file(
    root: &Path,
    current_relative_path: &str,
    next_file_name: &str,
) -> Result<(String, String, String), String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let current_path = resolve_existing_file_inside_root(&canonical_root, current_relative_path)?;

    if !is_text_document_file(&current_path) {
        return Err("只能重命名 TXT 文件。".to_owned());
    }

    let safe_file_name = validate_text_document_file_name(next_file_name)?;
    let target_path = current_path.with_file_name(safe_file_name);
    let target_parent = target_path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let canonical_target_parent =
        fs::canonicalize(target_parent).map_err(|error| format!("无法解析目标父目录：{error}"))?;

    // 目标父目录必须仍在知识库内，防止通过异常路径或符号链接逃逸。
    if !canonical_target_parent.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止重命名。".to_owned());
    }

    if target_path.exists() {
        return Err("目标文件名已存在，已阻止覆盖。".to_owned());
    }

    fs::rename(&current_path, &target_path)
        .map_err(|error| format!("无法重命名 TXT 文件：{error}"))?;

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取重命名后的 TXT 文件：{error}"))?;
    let canonical_target = fs::canonicalize(&target_path)
        .map_err(|error| format!("无法解析重命名后的文件：{error}"))?;
    let next_relative_path = canonical_target
        .strip_prefix(&canonical_root)
        .map_err(|error| format!("无法计算重命名后的相对路径：{error}"))?
        .to_string_lossy()
        .replace('\\', "/");
    let current_hash = hash_content(&current_content);

    Ok((next_relative_path, current_content, current_hash))
}

/** 将 Markdown 文件移入系统回收站，删除前用 hash 确认没有外部修改。 */
pub fn trash_markdown_file(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
) -> Result<(), String> {
    trash_markdown_file_with(root, relative_path, expected_hash, |target_path| {
        trash::delete(target_path).map_err(|error| format!("无法移入系统回收站：{error}"))
    })
}

/** 执行删除前统一校验，真实运行时注入系统回收站删除器，测试中注入可控删除器。 */
pub(crate) fn trash_markdown_file_with<F>(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
    delete_file: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let target_path = resolve_existing_file_inside_root(root, relative_path)?;

    if !is_markdown_file(&target_path) {
        return Err("只能删除 Markdown 文件。".to_owned());
    }

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取待删除 Markdown 文件：{error}"))?;
    let current_hash = hash_content(&current_content);

    // 删除是破坏性操作，即使进入回收站也要先确认文件版本没有被外部编辑器改动。
    if current_hash != expected_hash {
        return Err("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。".to_owned());
    }

    delete_file(&target_path)
}

/** 将 txt 文档移入系统回收站，删除前用 hash 确认没有外部修改。 */
pub fn trash_text_document_file(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
) -> Result<(), String> {
    trash_text_document_file_with(root, relative_path, expected_hash, |target_path| {
        trash::delete(target_path).map_err(|error| format!("无法移入系统回收站：{error}"))
    })
}

/** 执行 txt 删除前统一校验，测试中可注入可控删除器。 */
pub(crate) fn trash_text_document_file_with<F>(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
    delete_file: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let target_path = resolve_existing_file_inside_root(root, relative_path)?;

    if !is_text_document_file(&target_path) {
        return Err("只能删除 TXT 文件。".to_owned());
    }

    let current_content = fs::read_to_string(&target_path)
        .map_err(|error| format!("无法读取待删除 TXT 文件：{error}"))?;
    let current_hash = hash_content(&current_content);

    // 删除是破坏性操作，即使进入回收站也要先确认文件版本没有被外部编辑器改动。
    if current_hash != expected_hash {
        return Err("目标文件已被外部修改，已阻止删除。请重新扫描后再操作。".to_owned());
    }

    delete_file(&target_path)
}

/** 原子写入 Markdown 文件，避免写到一半时破坏用户数据。 */
pub fn atomic_write_markdown(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let mut temp_file =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时文件：{error}"))?;

    temp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("无法写入临时文件：{error}"))?;
    temp_file
        .persist(path)
        .map_err(|error| format!("无法替换 Markdown 文件：{}", error.error))?;

    Ok(())
}

/** 原子写入 txt 文档，保持和 Markdown 保存相同的本地数据安全语义。 */
pub fn atomic_write_text_document(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "目标路径缺少父目录".to_owned())?;
    let mut temp_file =
        NamedTempFile::new_in(parent).map_err(|error| format!("无法创建临时文件：{error}"))?;

    temp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("无法写入临时文件：{error}"))?;
    temp_file
        .persist(path)
        .map_err(|error| format!("无法替换 TXT 文件：{}", error.error))?;

    Ok(())
}

/** 加载 docx/pdf/图片的只读预览数据，txt 不走该接口。 */
pub fn load_document_preview(
    root: &Path,
    document: &WorkspaceDocument,
) -> Result<DocumentPreview, String> {
    let target_path = resolve_existing_file_inside_root(root, &document.path)?;

    match document.file_type.as_str() {
        "pdf" => {
            if !is_pdf_document_file(&target_path) {
                return Err("只能预览 PDF 文件。".to_owned());
            }

            let bytes = fs::read(&target_path)
                .map_err(|error| format!("无法读取 PDF 文件 {}：{error}", target_path.display()))?;
            // 预览同步返回本地文本层；iframe 继续负责保留 PDF 的原始版式。
            // 文本层解析失败不应阻断原始 PDF iframe 预览；Agent 读取时才返回明确失败原因。
            let blocks = extract_document_text(root, document)
                .map(|extraction| {
                    extraction
                        .blocks
                        .into_iter()
                        .map(|block| DocumentPreviewBlock {
                            r#type: "paragraph".to_owned(),
                            text: block.text,
                            page: block.page,
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(DocumentPreview {
                document_id: document.id.clone(),
                file_type: "pdf".to_owned(),
                title: document.title.clone(),
                path: document.path.clone(),
                updated_at: "刚刚".to_owned(),
                content_hash: hash_bytes(&bytes),
                asset_path: Some(target_path.to_string_lossy().to_string()),
                blocks: Some(blocks),
            })
        }
        "docx" => {
            if !is_docx_document_file(&target_path) {
                return Err("只能预览 DOCX 文件。".to_owned());
            }

            let bytes = fs::read(&target_path).map_err(|error| {
                format!("无法读取 DOCX 文件 {}：{error}", target_path.display())
            })?;
            let blocks = extract_docx_preview_blocks(&target_path)?;

            Ok(DocumentPreview {
                document_id: document.id.clone(),
                file_type: "docx".to_owned(),
                title: document.title.clone(),
                path: document.path.clone(),
                updated_at: "刚刚".to_owned(),
                content_hash: hash_bytes(&bytes),
                asset_path: None,
                blocks: Some(blocks),
            })
        }
        "image" => {
            if !is_image_document_file(&target_path) {
                return Err("只能预览图片文件。".to_owned());
            }

            let bytes = fs::read(&target_path)
                .map_err(|error| format!("无法读取图片文件 {}：{error}", target_path.display()))?;

            Ok(DocumentPreview {
                document_id: document.id.clone(),
                file_type: "image".to_owned(),
                title: document.title.clone(),
                path: document.path.clone(),
                updated_at: "刚刚".to_owned(),
                content_hash: hash_bytes(&bytes),
                asset_path: Some(target_path.to_string_lossy().to_string()),
                blocks: None,
            })
        }
        _ => Err("该文档类型不支持只读预览。".to_owned()),
    }
}

/** 从 docx 的 word/document.xml 中抽取段落级文本块。 */
pub fn extract_docx_preview_blocks(path: &Path) -> Result<Vec<DocumentPreviewBlock>, String> {
    let file = fs::File::open(path).map_err(|error| format!("无法打开 DOCX 文件：{error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("DOCX 文件结构无效：{error}"))?;
    let mut document_xml = String::new();

    archive
        .by_name("word/document.xml")
        .map_err(|error| format!("DOCX 缺少正文 XML：{error}"))?
        .read_to_string(&mut document_xml)
        .map_err(|error| format!("无法读取 DOCX 正文 XML：{error}"))?;

    parse_docx_document_xml(&document_xml)
}

/** 解析 WordprocessingML 正文；todo: 后续补充表格、图片、批注和样式的高保真还原。 */
pub(crate) fn parse_docx_document_xml(
    document_xml: &str,
) -> Result<Vec<DocumentPreviewBlock>, String> {
    let mut reader = Reader::from_str(document_xml);
    let mut blocks = Vec::new();
    let mut buffer = Vec::new();
    let mut in_paragraph = false;
    // 表格单元格内的段落仍按顺序保留，但以 table 类型交给预览和 Agent 说明其来源。
    let mut table_depth = 0usize;
    let mut in_text = false;
    let mut paragraph_text = String::new();
    let mut paragraph_style = String::new();

    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) => {
                let name = element.name();
                let name_bytes = name.as_ref();

                if xml_name_matches(name_bytes, b"p") {
                    in_paragraph = true;
                    paragraph_text.clear();
                    paragraph_style.clear();
                } else if xml_name_matches(name_bytes, b"tbl") {
                    table_depth = table_depth.saturating_add(1);
                } else if in_paragraph && xml_name_matches(name_bytes, b"t") {
                    in_text = true;
                } else if in_paragraph && xml_name_matches(name_bytes, b"pStyle") {
                    if let Some(style) = read_xml_attribute(&element, b"val") {
                        paragraph_style = style;
                    }
                }
            }
            Ok(Event::Empty(element)) => {
                let name = element.name();
                let name_bytes = name.as_ref();

                if in_paragraph && xml_name_matches(name_bytes, b"pStyle") {
                    if let Some(style) = read_xml_attribute(&element, b"val") {
                        paragraph_style = style;
                    }
                } else if in_paragraph && xml_name_matches(name_bytes, b"tab") {
                    paragraph_text.push('\t');
                } else if in_paragraph && xml_name_matches(name_bytes, b"br") {
                    paragraph_text.push('\n');
                }
            }
            Ok(Event::Text(text)) => {
                if in_text {
                    let raw_text = std::str::from_utf8(text.as_ref())
                        .map_err(|error| format!("DOCX 文本编码无效：{error}"))?;
                    let unescaped_text = quick_xml::escape::unescape(raw_text)
                        .map_err(|error| format!("DOCX 文本转义无效：{error}"))?;

                    paragraph_text.push_str(&unescaped_text);
                }
            }
            Ok(Event::End(element)) => {
                let name = element.name();
                let name_bytes = name.as_ref();

                if xml_name_matches(name_bytes, b"t") {
                    in_text = false;
                } else if xml_name_matches(name_bytes, b"p") {
                    let trimmed_text = paragraph_text.trim();

                    if !trimmed_text.is_empty() {
                        blocks.push(DocumentPreviewBlock {
                            r#type: if table_depth > 0 {
                                "table".to_owned()
                            } else if is_docx_heading_style(&paragraph_style) {
                                "heading".to_owned()
                            } else {
                                "paragraph".to_owned()
                            },
                            text: trimmed_text.to_owned(),
                            page: None,
                        });
                    }

                    in_paragraph = false;
                    in_text = false;
                    paragraph_text.clear();
                    paragraph_style.clear();
                } else if xml_name_matches(name_bytes, b"tbl") {
                    table_depth = table_depth.saturating_sub(1);
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("DOCX 正文 XML 解析失败：{error}")),
            _ => {}
        }

        buffer.clear();
    }

    if blocks.is_empty() {
        blocks.push(DocumentPreviewBlock {
            r#type: "paragraph".to_owned(),
            text: "该 DOCX 暂无可预览正文。".to_owned(),
            page: None,
        });
    }

    Ok(blocks)
}

/**
 * 按需抽取已授权 DOCX/PDF 的结构化文本，供预览与 Agent 读取复用。
 *
 * 解析只读取知识库内已校验路径，不将正文写回原文件或 WorkspaceSnapshot。
 */
pub fn extract_document_text(
    root: &Path,
    document: &WorkspaceDocument,
) -> Result<DocumentTextExtraction, String> {
    let target_path = resolve_existing_file_inside_root(root, &document.path)?;
    let bytes = fs::read(&target_path).map_err(|error| format!("无法读取文档内容：{error}"))?;
    let content_hash = hash_bytes(&bytes);

    let (blocks, warnings) = match document.file_type.as_str() {
        "docx" => {
            if !is_docx_document_file(&target_path) {
                return Err("只能读取 DOCX 文件。".to_owned());
            }
            let preview_blocks = extract_docx_preview_blocks(&target_path)?;
            let has_empty_placeholder =
                preview_blocks.len() == 1 && preview_blocks[0].text == "该 DOCX 暂无可预览正文。";
            let blocks = if has_empty_placeholder {
                Vec::new()
            } else {
                preview_blocks
                    .into_iter()
                    .enumerate()
                    .map(|(index, block)| DocumentTextBlock {
                        index: index + 1,
                        r#type: block.r#type,
                        text: block.text,
                        page: None,
                    })
                    .collect()
            };
            let warnings = if blocks.is_empty() {
                vec!["未从 DOCX 正文提取到可读文本；图片、文本框和批注不会自动识别。".to_owned()]
            } else {
                vec![
                    "DOCX 首期仅提取正文段落和表格文本；图片、文本框、批注和修订可能未包含。"
                        .to_owned(),
                ]
            };
            (blocks, warnings)
        }
        "pdf" => {
            if !is_pdf_document_file(&target_path) {
                return Err("只能读取 PDF 文件。".to_owned());
            }
            let extracted = pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|error| format!("PDF 文本提取失败：{error}"))?;
            // pdf-extract 通常用换页符分隔页面；缺失时仍把结果作为第一页，避免丢失文本。
            let blocks: Vec<DocumentTextBlock> = extracted
                .split('\u{c}')
                .enumerate()
                .filter_map(|(index, page_text)| {
                    let text = page_text.trim();
                    (!text.is_empty()).then(|| DocumentTextBlock {
                        index: index + 1,
                        r#type: "page".to_owned(),
                        text: text.to_owned(),
                        page: Some(index + 1),
                    })
                })
                .collect();
            let warnings = if blocks.is_empty() {
                vec!["未从 PDF 提取到文本；这可能是扫描件或受保护文档。可在对话中明确要求读取页面图片。".to_owned()]
            } else {
                Vec::new()
            };
            (blocks, warnings)
        }
        _ => return Err("该文档类型不支持内容读取。".to_owned()),
    };
    let content_chars = blocks.iter().map(|block| block.text.chars().count()).sum();

    log::info!(
        target: "document_extraction",
        "文档文本抽取完成：type={} path={} blocks={} chars={} warnings={}",
        document.file_type,
        document.path,
        blocks.len(),
        content_chars,
        warnings.len()
    );
    Ok(DocumentTextExtraction {
        document_id: document.id.clone(),
        file_type: document.file_type.clone(),
        content_hash,
        blocks,
        content_chars,
        warnings,
    })
}

/** 判断带命名空间或不带命名空间的 XML 名称是否匹配目标本地名。 */
pub(crate) fn xml_name_matches(name: &[u8], local_name: &[u8]) -> bool {
    name == local_name
        || name
            .strip_prefix(b"w:")
            .is_some_and(|stripped_name| stripped_name == local_name)
        || name
            .rsplit(|byte| *byte == b':')
            .next()
            .is_some_and(|stripped_name| stripped_name == local_name)
}

/** 读取 XML 属性，兼容 WordprocessingML 的 w:val 命名空间前缀。 */
pub(crate) fn read_xml_attribute(
    element: &quick_xml::events::BytesStart<'_>,
    local_name: &[u8],
) -> Option<String> {
    element
        .attributes()
        .filter_map(Result::ok)
        .find_map(|attribute| {
            xml_name_matches(attribute.key.as_ref(), local_name)
                .then(|| String::from_utf8_lossy(attribute.value.as_ref()).to_string())
        })
}

/** 根据 docx 段落样式判断是否应作为标题展示。 */
pub(crate) fn is_docx_heading_style(style: &str) -> bool {
    let normalized_style = style.to_ascii_lowercase();

    normalized_style.contains("heading") || normalized_style.contains("title")
}
