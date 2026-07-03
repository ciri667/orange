use crate::domain::{
    DocumentPreviewBlock, ExportCurrentFilePayload, ExportFileResult, ExportFormat,
    ExportTargetKind, KnowledgeBase, Note, WorkspaceDocument,
};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage;
use genpdf::{elements, fonts, style, Alignment, Element as _};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

/** 生成阅读版 PDF 时使用的默认标题字号。 */
const PDF_TITLE_FONT_SIZE: u8 = 20;

/** 生成阅读版 PDF 时使用的普通正文字号。 */
const PDF_BODY_FONT_SIZE: u8 = 11;

/** 保存对话框关闭后的状态，区分用户取消和真实导出失败。 */
#[derive(Clone, Debug)]
enum ExportOutcome {
    Cancelled,
    Completed(ExportFileResult),
}

/** 已解析的导出源文件，统一封装 note/document 的路径、类型和导出内容。 */
#[derive(Clone, Debug)]
struct ExportSource {
    entity_type: &'static str,
    entity_id: String,
    title: String,
    relative_path: String,
    source_type: ExportSourceType,
    knowledge_base: KnowledgeBase,
    content: ExportContent,
}

/** 导出源文档类型，用于格式矩阵、建议文件名和日志元数据。 */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportSourceType {
    Markdown,
    Txt,
    Docx,
    Pdf,
    Image,
}

impl ExportSourceType {
    /** 返回脱敏日志和前端提示都能使用的稳定类型标识。 */
    fn as_str(self) -> &'static str {
        match self {
            ExportSourceType::Markdown => "markdown",
            ExportSourceType::Txt => "txt",
            ExportSourceType::Docx => "docx",
            ExportSourceType::Pdf => "pdf",
            ExportSourceType::Image => "image",
        }
    }
}

/** 导出所需内容；二进制源文件不预读，避免大文件进入内存和日志。 */
#[derive(Clone, Debug)]
enum ExportContent {
    Text(String),
    DocxBlocks(Vec<DocumentPreviewBlock>),
    BinaryFile,
}

/** 文件导出命令入口；返回 None 表示用户取消系统保存对话框。 */
#[tauri::command]
pub async fn export_current_file(
    app: AppHandle,
    payload: ExportCurrentFilePayload,
) -> Result<Option<ExportFileResult>, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("export");
    let requested_format = payload.format;
    let source = resolve_export_source(payload)?;
    let target_extension = resolve_export_extension(&source, requested_format)?;
    let suggested_file_name = build_export_file_name(&source, &target_extension);

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Editor,
            "export_file",
            "started",
            "开始导出当前文件。",
        )
        .operation_id(operation_id.clone())
        .knowledge_base_id(source.knowledge_base.id.clone())
        .entity(source.entity_type, source.entity_id.clone())
        .relative_path(source.relative_path.clone())
        .metadata(json!({
            "format": export_format_name(requested_format),
            "sourceType": source.source_type.as_str(),
        })),
    );

    let result = export_current_file_inner(
        &app,
        &source,
        requested_format,
        &target_extension,
        &suggested_file_name,
    )
    .await;

    match &result {
        Ok(ExportOutcome::Completed(result)) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Info,
                    AppLogCategory::Editor,
                    "export_file",
                    "completed",
                    "当前文件导出完成。",
                )
                .operation_id(operation_id)
                .knowledge_base_id(source.knowledge_base.id.clone())
                .entity(source.entity_type, source.entity_id.clone())
                .relative_path(source.relative_path.clone())
                .duration(started_at.elapsed())
                .metadata(json!({
                    "format": export_format_name(requested_format),
                    "sourceType": source.source_type.as_str(),
                    "byteSize": result.byte_size,
                })),
            );

            Ok(Some(result.clone()))
        }
        Ok(ExportOutcome::Cancelled) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Info,
                    AppLogCategory::Editor,
                    "export_file",
                    "cancelled",
                    "用户取消当前文件导出。",
                )
                .operation_id(operation_id)
                .knowledge_base_id(source.knowledge_base.id.clone())
                .entity(source.entity_type, source.entity_id.clone())
                .relative_path(source.relative_path.clone())
                .duration(started_at.elapsed())
                .metadata(json!({
                    "format": export_format_name(requested_format),
                    "sourceType": source.source_type.as_str(),
                })),
            );

            Ok(None)
        }
        Err(error) => {
            logging::write_app_event_best_effort(
                &app,
                AppEventBuilder::new(
                    AppLogLevel::Error,
                    AppLogCategory::Editor,
                    "export_file",
                    "failed",
                    "当前文件导出失败。",
                )
                .operation_id(operation_id)
                .knowledge_base_id(source.knowledge_base.id.clone())
                .entity(source.entity_type, source.entity_id.clone())
                .relative_path(source.relative_path.clone())
                .duration(started_at.elapsed())
                .metadata(json!({
                    "format": export_format_name(requested_format),
                    "sourceType": source.source_type.as_str(),
                })),
            );

            Err(error.clone())
        }
    }
}

/** 执行保存对话框和文件写入；调用方负责统一记录成功、取消和失败日志。 */
async fn export_current_file_inner(
    app: &AppHandle,
    source: &ExportSource,
    format: ExportFormat,
    target_extension: &str,
    suggested_file_name: &str,
) -> Result<ExportOutcome, String> {
    let Some(selected_path) =
        pick_export_file_path(app, suggested_file_name, target_extension).await?
    else {
        return Ok(ExportOutcome::Cancelled);
    };
    let target_path = normalize_export_target_path(selected_path, target_extension);
    let source_path = storage::resolve_existing_file_inside_root(
        PathBuf::from(&source.knowledge_base.path).as_path(),
        &source.relative_path,
    )?;

    // 选择原文件或 PDF->PDF 时只复制源文件，避免无意义地解析大 PDF。
    if should_copy_original(source.source_type, format) {
        copy_export_file(&source_path, &target_path)?;
    } else if matches!(format, ExportFormat::Markdown) {
        let markdown = render_markdown_export(source, &source_path)?;

        fs::write(&target_path, markdown)
            .map_err(|error| format!("无法写入 Markdown 导出文件：{error}"))?;
    } else {
        let markdown = render_markdown_export(source, &source_path)?;

        write_reading_pdf(&target_path, &source.title, &markdown)?;
    }

    let metadata =
        fs::metadata(&target_path).map_err(|error| format!("无法读取导出文件元数据：{error}"))?;
    let file_name = target_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(suggested_file_name)
        .to_owned();

    Ok(ExportOutcome::Completed(ExportFileResult {
        format,
        target_path: target_path.to_string_lossy().to_string(),
        file_name,
        byte_size: metadata.len(),
    }))
}

/** 从命令 payload 中定位当前 note/document，并构造后续导出所需的统一模型。 */
fn resolve_export_source(payload: ExportCurrentFilePayload) -> Result<ExportSource, String> {
    match payload.target_kind {
        ExportTargetKind::Note => {
            let note = payload
                .snapshot
                .notes
                .iter()
                .find(|note| note.id == payload.target_id)
                .cloned()
                .ok_or_else(|| "找不到要导出的 Markdown 笔记。".to_owned())?;
            let knowledge_base =
                find_knowledge_base(&payload.snapshot.knowledge_bases, &note.knowledge_base_id)?;

            Ok(source_from_note(note, knowledge_base))
        }
        ExportTargetKind::Document => {
            let document = payload
                .snapshot
                .documents
                .iter()
                .find(|document| document.id == payload.target_id)
                .cloned()
                .ok_or_else(|| "找不到要导出的文档。".to_owned())?;
            let knowledge_base = find_knowledge_base(
                &payload.snapshot.knowledge_bases,
                &document.knowledge_base_id,
            )?;

            source_from_document(document, knowledge_base)
        }
    }
}

/** 根据知识库 ID 查找导出源所属根目录。 */
fn find_knowledge_base(
    knowledge_bases: &[KnowledgeBase],
    knowledge_base_id: &str,
) -> Result<KnowledgeBase, String> {
    knowledge_bases
        .iter()
        .find(|knowledge_base| knowledge_base.id == knowledge_base_id)
        .cloned()
        .ok_or_else(|| "找不到导出文件所属知识库。".to_owned())
}

/** 将 Markdown 笔记转换成统一导出源。 */
fn source_from_note(note: Note, knowledge_base: KnowledgeBase) -> ExportSource {
    ExportSource {
        entity_type: "note",
        entity_id: note.id,
        title: note.title,
        relative_path: note.path,
        source_type: ExportSourceType::Markdown,
        knowledge_base,
        content: ExportContent::Text(note.content),
    }
}

/** 将普通文档转换成统一导出源，并拒绝未知文档类型。 */
fn source_from_document(
    document: WorkspaceDocument,
    knowledge_base: KnowledgeBase,
) -> Result<ExportSource, String> {
    let source_type = match document.file_type.as_str() {
        "txt" => ExportSourceType::Txt,
        "docx" => ExportSourceType::Docx,
        "pdf" => ExportSourceType::Pdf,
        "image" => ExportSourceType::Image,
        _ => return Err("该文档类型暂不支持导出。".to_owned()),
    };
    let content = match source_type {
        ExportSourceType::Txt => ExportContent::Text(document.content.unwrap_or_default()),
        ExportSourceType::Docx => ExportContent::DocxBlocks(Vec::new()),
        ExportSourceType::Pdf | ExportSourceType::Image => ExportContent::BinaryFile,
        ExportSourceType::Markdown => ExportContent::Text(String::new()),
    };

    Ok(ExportSource {
        entity_type: "document",
        entity_id: document.id,
        title: document.title,
        relative_path: document.path,
        source_type,
        knowledge_base,
        content,
    })
}

/** 根据源类型和用户选择校验格式矩阵，并返回目标扩展名。 */
fn resolve_export_extension(source: &ExportSource, format: ExportFormat) -> Result<String, String> {
    match format {
        ExportFormat::Original => original_extension_from_source(source),
        ExportFormat::Markdown if matches!(&source.content, ExportContent::BinaryFile) => {
            Err("二进制文件暂不支持转为 Markdown。".to_owned())
        }
        ExportFormat::Markdown => Ok("md".to_owned()),
        ExportFormat::Pdf if source.source_type == ExportSourceType::Image => {
            Err("图片暂不支持转为 PDF。".to_owned())
        }
        ExportFormat::Pdf => Ok("pdf".to_owned()),
    }
}

/** 原文件导出尽量沿用知识库内真实扩展名，图片等多扩展类型不会被统一改名。 */
fn original_extension_from_source(source: &ExportSource) -> Result<String, String> {
    Path::new(&source.relative_path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .filter(|extension| !extension.is_empty())
        .ok_or_else(|| "无法识别原文件扩展名。".to_owned())
}

/** 判断当前导出是否可以直接复制源文件。 */
fn should_copy_original(source_type: ExportSourceType, format: ExportFormat) -> bool {
    matches!(format, ExportFormat::Original)
        || (source_type == ExportSourceType::Pdf && matches!(format, ExportFormat::Pdf))
}

/** 打开系统保存对话框；默认路径指向用户下载目录并附带建议文件名。 */
async fn pick_export_file_path(
    app: &AppHandle,
    suggested_file_name: &str,
    extension: &str,
) -> Result<Option<PathBuf>, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel(1);
    let download_dir = app
        .path()
        .download_dir()
        .map_err(|error| format!("无法定位下载目录：{error}"))?;

    app.dialog()
        .file()
        .set_title("导出当前文件")
        .set_directory(download_dir)
        .set_file_name(suggested_file_name)
        .add_filter(format!("{} 文件", extension.to_uppercase()), &[extension])
        .save_file(move |selected_path| {
            let _ = sender.blocking_send(selected_path);
        });

    Ok(receiver
        .recv()
        .await
        .flatten()
        .and_then(|path| path.as_path().map(PathBuf::from)))
}

/** 对用户选择路径补齐目标扩展名，扩展不匹配时改成目标格式扩展。 */
fn normalize_export_target_path(mut selected_path: PathBuf, extension: &str) -> PathBuf {
    let normalized_extension = extension.trim_start_matches('.').to_ascii_lowercase();
    let current_extension = selected_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);

    if current_extension.as_deref() != Some(normalized_extension.as_str()) {
        selected_path.set_extension(normalized_extension);
    }

    selected_path
}

/** 复制原始文件到用户选择路径；fs::copy 走系统路径，不把文件内容读入日志。 */
fn copy_export_file(source_path: &Path, target_path: &Path) -> Result<(), String> {
    if source_path == target_path {
        return Err("导出目标不能与原文件相同。".to_owned());
    }

    fs::copy(source_path, target_path).map_err(|error| format!("无法复制原文件：{error}"))?;

    Ok(())
}

/** 渲染 Markdown 导出内容；DOCX 会在需要转换时延迟读取源文件块。 */
fn render_markdown_export(source: &ExportSource, source_path: &Path) -> Result<String, String> {
    match &source.content {
        ExportContent::Text(content) => Ok(content.clone()),
        ExportContent::DocxBlocks(blocks) if !blocks.is_empty() => {
            Ok(docx_blocks_to_markdown(blocks))
        }
        ExportContent::DocxBlocks(_) => {
            let blocks = storage::extract_docx_preview_blocks(source_path)?;

            Ok(docx_blocks_to_markdown(&blocks))
        }
        ExportContent::BinaryFile => Err("PDF 暂不支持转为 Markdown。".to_owned()),
    }
}

/** 将 DOCX 预览块转换成保守 Markdown，只保留标题和段落。 */
fn docx_blocks_to_markdown(blocks: &[DocumentPreviewBlock]) -> String {
    blocks
        .iter()
        .map(|block| {
            if block.r#type == "heading" {
                format!("## {}", block.text.trim())
            } else {
                block.text.trim().to_owned()
            }
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/** 生成阅读版 PDF，保留 Markdown 的基础标题、列表、代码块和段落结构。 */
fn write_reading_pdf(target_path: &Path, title: &str, markdown: &str) -> Result<(), String> {
    let font_family = load_pdf_font_family()?;
    let mut document = genpdf::Document::new(font_family);
    let mut decorator = genpdf::SimplePageDecorator::new();

    decorator.set_margins(16);
    document.set_title(title);
    document.set_minimal_conformance();
    document.set_line_spacing(1.25);
    document.set_page_decorator(decorator);

    document.push(
        elements::Paragraph::new(title)
            .aligned(Alignment::Left)
            .styled(
                style::Style::new()
                    .bold()
                    .with_font_size(PDF_TITLE_FONT_SIZE),
            ),
    );
    document.push(elements::Break::new(1));

    for block in parse_markdown_blocks(markdown) {
        push_pdf_block(&mut document, block);
    }

    document
        .render_to_file(target_path)
        .map_err(|error| format!("无法生成 PDF 文件：{error}"))
}

/** 从常见系统字体中找到一个 rusttype 可读取的中英文字体。 */
fn load_pdf_font_family() -> Result<fonts::FontFamily<fonts::FontData>, String> {
    for path in candidate_pdf_font_paths() {
        if !path.exists() || !path.is_file() {
            continue;
        }

        // 同一个字体文件复用到粗体/斜体变体，保证中文可读；样式由 PDF 生成器尽力处理。
        if let Ok(font_data) = fonts::FontData::load(&path, None) {
            return Ok(fonts::FontFamily {
                regular: font_data.clone(),
                bold: font_data.clone(),
                italic: font_data.clone(),
                bold_italic: font_data,
            });
        }
    }

    Err("未找到可用于 PDF 导出的中英文字体，请安装常见 TTF/OTF 字体后重试。".to_owned())
}

/** 返回按平台排序的字体候选路径；绝对路径只用于读取字体，不进入日志。 */
fn candidate_pdf_font_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
        PathBuf::from("/System/Library/Fonts/STHeiti Medium.ttc"),
        PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
        PathBuf::from("/Library/Fonts/Arial Unicode.ttf"),
        PathBuf::from("C:/Windows/Fonts/msyh.ttc"),
        PathBuf::from("C:/Windows/Fonts/simhei.ttf"),
        PathBuf::from("C:/Windows/Fonts/arial.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
    ]
}

/** 阅读版 PDF 的中间块模型，避免直接把 Markdown 字符串塞进单一段落。 */
#[derive(Clone, Debug, PartialEq, Eq)]
enum PdfBlock {
    Heading {
        level: usize,
        text: String,
    },
    Paragraph(String),
    ListItem {
        ordered: bool,
        index: usize,
        text: String,
    },
    Code(String),
}

/** 轻量解析 Markdown 结构；todo: 后续可接入完整 Markdown AST 以支持表格和引用块。 */
fn parse_markdown_blocks(markdown: &str) -> Vec<PdfBlock> {
    let mut blocks = Vec::new();
    let mut paragraph_lines = Vec::new();
    let mut code_lines = Vec::new();
    let mut in_code_block = false;

    for raw_line in markdown.lines() {
        let line = raw_line.trim_end();

        if line.trim_start().starts_with("```") {
            if in_code_block {
                blocks.push(PdfBlock::Code(code_lines.join("\n")));
                code_lines.clear();
                in_code_block = false;
            } else {
                flush_paragraph(&mut blocks, &mut paragraph_lines);
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            code_lines.push(line.to_owned());
            continue;
        }

        let trimmed = line.trim();

        if trimmed.is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            blocks.push(PdfBlock::Heading { level, text });
            continue;
        }

        if let Some((ordered, index, text)) = parse_list_item(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph_lines);
            blocks.push(PdfBlock::ListItem {
                ordered,
                index,
                text,
            });
            continue;
        }

        paragraph_lines.push(trimmed.to_owned());
    }

    if in_code_block && !code_lines.is_empty() {
        blocks.push(PdfBlock::Code(code_lines.join("\n")));
    }

    flush_paragraph(&mut blocks, &mut paragraph_lines);

    blocks
}

/** 将累计段落行压入块列表，Markdown 换行在阅读版 PDF 中合并为空格。 */
fn flush_paragraph(blocks: &mut Vec<PdfBlock>, paragraph_lines: &mut Vec<String>) {
    if paragraph_lines.is_empty() {
        return;
    }

    blocks.push(PdfBlock::Paragraph(paragraph_lines.join(" ")));
    paragraph_lines.clear();
}

/** 解析 Markdown 标题，最多支持六级标题。 */
fn parse_heading(line: &str) -> Option<(usize, String)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();

    if !(1..=6).contains(&level)
        || !line
            .chars()
            .nth(level)
            .is_some_and(|character| character.is_whitespace())
    {
        return None;
    }

    Some((level, line[level..].trim().to_owned()))
}

/** 解析有序和无序列表项，返回是否有序、序号和正文。 */
fn parse_list_item(line: &str) -> Option<(bool, usize, String)> {
    if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, 0, text.trim().to_owned()));
    }

    let dot_index = line.find('.')?;
    let (prefix, suffix_with_dot) = line.split_at(dot_index);
    let suffix = suffix_with_dot.strip_prefix('.')?.trim_start();

    if prefix.is_empty()
        || suffix.is_empty()
        || !prefix.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }

    Some((true, prefix.parse().unwrap_or(1), suffix.to_owned()))
}

/** 将单个解析块写入 PDF 文档。 */
fn push_pdf_block(document: &mut genpdf::Document, block: PdfBlock) {
    match block {
        PdfBlock::Heading { level, text } => {
            let font_size = match level {
                1 => 18,
                2 => 16,
                3 => 14,
                _ => 12,
            };

            document.push(elements::Break::new(0.5));
            document.push(
                elements::Paragraph::new(text)
                    .styled(style::Style::new().bold().with_font_size(font_size)),
            );
        }
        PdfBlock::Paragraph(text) => {
            document.push(
                elements::Paragraph::new(text)
                    .styled(style::Style::new().with_font_size(PDF_BODY_FONT_SIZE)),
            );
        }
        PdfBlock::ListItem {
            ordered,
            index,
            text,
        } => {
            let prefix = if ordered {
                format!("{index}. ")
            } else {
                "• ".to_owned()
            };

            document.push(
                elements::Paragraph::new(format!("{prefix}{text}"))
                    .styled(style::Style::new().with_font_size(PDF_BODY_FONT_SIZE)),
            );
        }
        PdfBlock::Code(code) => {
            let normalized_code = if code.trim().is_empty() {
                " ".to_owned()
            } else {
                code
            };

            document.push(
                elements::Paragraph::new(normalized_code)
                    .framed()
                    .padded(genpdf::Margins::trbl(1, 1, 1, 1))
                    .styled(style::Style::new().with_font_size(10)),
            );
        }
    }
}

/** 构建保存对话框建议文件名，保留原标题但移除跨平台不安全字符。 */
fn build_export_file_name(source: &ExportSource, extension: &str) -> String {
    let base_name = sanitize_export_file_stem(&source.title)
        .or_else(|| file_stem_from_relative_path(&source.relative_path))
        .unwrap_or_else(|| "导出文件".to_owned());

    format!("{base_name}.{extension}")
}

/** 提取知识库内相对路径的文件 stem，用于标题为空时兜底命名。 */
fn file_stem_from_relative_path(relative_path: &str) -> Option<String> {
    Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(sanitize_export_file_stem)
}

/** 清理导出文件名 stem，避免保存对话框收到路径分隔符或系统保留字符。 */
fn sanitize_export_file_stem(stem: &str) -> Option<String> {
    let sanitized = stem
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>()
        .trim_matches('.')
        .trim()
        .to_owned();

    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

/** 返回导出格式的稳定字符串，避免日志直接序列化 enum 形态漂移。 */
fn export_format_name(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Original => "original",
        ExportFormat::Markdown => "markdown",
        ExportFormat::Pdf => "pdf",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_export_file_name, docx_blocks_to_markdown, normalize_export_target_path,
        parse_markdown_blocks, render_markdown_export, resolve_export_extension, ExportContent,
        ExportSource, ExportSourceType, PdfBlock,
    };
    use crate::domain::{DocumentPreviewBlock, ExportFormat, KnowledgeBase};
    use crate::storage;
    use std::fs;
    use std::path::{Path, PathBuf};

    /** 构造测试用导出源，避免每个测试重复铺开知识库字段。 */
    fn test_source(source_type: ExportSourceType, content: ExportContent) -> ExportSource {
        ExportSource {
            entity_type: "document",
            entity_id: "document-a".to_owned(),
            title: "A/B:标题".to_owned(),
            relative_path: "docs/source.txt".to_owned(),
            source_type,
            knowledge_base: KnowledgeBase {
                id: "kb-a".to_owned(),
                name: "KB".to_owned(),
                path: "/tmp/kb".to_owned(),
                description: String::new(),
                status: "ready".to_owned(),
                note_count: 0,
                document_count: 1,
                updated_at: "now".to_owned(),
                is_default: true,
                semantic_index_enabled: false,
                scan_report: None,
            },
            content,
        }
    }

    #[test]
    fn export_extension_rejects_pdf_to_markdown() {
        let pdf_source = test_source(ExportSourceType::Pdf, ExportContent::BinaryFile);
        let txt_source = test_source(
            ExportSourceType::Txt,
            ExportContent::Text("正文".to_owned()),
        );
        let result = resolve_export_extension(&pdf_source, ExportFormat::Markdown);

        assert!(result.is_err());
        assert_eq!(
            resolve_export_extension(&txt_source, ExportFormat::Markdown).unwrap(),
            "md"
        );
    }

    #[test]
    fn original_export_keeps_image_extension() {
        let mut source = test_source(ExportSourceType::Image, ExportContent::BinaryFile);

        source.relative_path = "assets/diagram.webp".to_owned();

        assert_eq!(
            resolve_export_extension(&source, ExportFormat::Original).unwrap(),
            "webp"
        );
        assert!(resolve_export_extension(&source, ExportFormat::Pdf).is_err());
    }

    #[test]
    fn target_path_extension_is_normalized() {
        assert_eq!(
            normalize_export_target_path(PathBuf::from("/tmp/out"), "md"),
            PathBuf::from("/tmp/out.md")
        );
        assert_eq!(
            normalize_export_target_path(PathBuf::from("/tmp/out.txt"), "pdf"),
            PathBuf::from("/tmp/out.pdf")
        );
    }

    #[test]
    fn suggested_file_name_sanitizes_path_characters() {
        let source = test_source(ExportSourceType::Txt, ExportContent::Text(String::new()));

        assert_eq!(build_export_file_name(&source, "md"), "A_B_标题.md");
    }

    #[test]
    fn docx_blocks_render_to_markdown() {
        let blocks = vec![
            DocumentPreviewBlock {
                r#type: "heading".to_owned(),
                text: "标题".to_owned(),
            },
            DocumentPreviewBlock {
                r#type: "paragraph".to_owned(),
                text: "正文".to_owned(),
            },
        ];

        assert_eq!(docx_blocks_to_markdown(&blocks), "## 标题\n\n正文");
    }

    #[test]
    fn markdown_parser_keeps_basic_structure() {
        let blocks =
            parse_markdown_blocks("# 标题\n\n段落一\n段落二\n\n- 项目\n\n```rs\nlet x = 1;\n```");

        assert_eq!(
            blocks,
            vec![
                PdfBlock::Heading {
                    level: 1,
                    text: "标题".to_owned()
                },
                PdfBlock::Paragraph("段落一 段落二".to_owned()),
                PdfBlock::ListItem {
                    ordered: false,
                    index: 0,
                    text: "项目".to_owned()
                },
                PdfBlock::Code("let x = 1;".to_owned())
            ]
        );
    }

    #[test]
    fn render_markdown_export_uses_text_content() {
        let source = test_source(
            ExportSourceType::Txt,
            ExportContent::Text("纯文本".to_owned()),
        );

        assert_eq!(
            render_markdown_export(&source, Path::new("/unused")).unwrap(),
            "纯文本"
        );
    }

    #[test]
    fn export_source_path_cannot_escape_knowledge_base_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside_path = dir
            .path()
            .parent()
            .unwrap()
            .join("outside-export-source.txt");

        fs::write(&outside_path, "outside").unwrap();

        // 导出源文件复用 storage 层边界校验，阻止 ../ 指向知识库根目录之外。
        let result =
            storage::resolve_existing_file_inside_root(dir.path(), "../outside-export-source.txt");

        assert!(result.is_err());
        let _ = fs::remove_file(outside_path);
    }

    #[test]
    fn reading_pdf_generation_writes_pdf_bytes_when_font_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("reading.pdf");

        match super::write_reading_pdf(
            &target_path,
            "中文标题",
            "# 中文标题\n\n- 项目\n\n```txt\n代码\n```",
        ) {
            Ok(()) => {
                let bytes = fs::read(&target_path).unwrap();

                assert!(bytes.starts_with(b"%PDF-"));
                assert!(bytes.len() > 100);
            }
            Err(error) if error.contains("未找到可用于 PDF 导出的中英文字体") => {
                // 无字体环境必须返回明确错误；有字体环境会走上面的真实生成 smoke test。
                assert!(error.contains("安装常见 TTF/OTF 字体"));
            }
            Err(error) => panic!("unexpected PDF generation error: {error}"),
        }
    }

    #[test]
    fn original_copy_preserves_source_content() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.txt");
        let target_path = dir.path().join("target.txt");

        fs::write(&source_path, "正文").unwrap();
        super::copy_export_file(&source_path, &target_path).unwrap();

        assert_eq!(fs::read_to_string(&source_path).unwrap(), "正文");
        assert_eq!(fs::read_to_string(&target_path).unwrap(), "正文");
        assert_eq!(
            fs::metadata(&target_path).unwrap().len(),
            "正文".len() as u64
        );
    }
}
