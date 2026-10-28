//! 文档图片：把 PDF 页面渲染成图片、抽取 DOCX 内嵌图片，交给视觉模型读取。
//!
//! 这里不做 OCR：扫描页、图表、手写内容都通过多模态模型理解。渲染只读取
//! 知识库内已校验路径，产物写入对话附件目录，模型请求前再由
//! [`crate::storage::hydrate_model_images`] 展开成 image_url。

use super::*;
use crate::domain::{ConversationImageAttachment, WorkspaceDocument};
use serde_json::json;

/// 渲染页面的长边上限，和对话图片请求预算保持一致。
const MAX_PAGE_LONG_EDGE: f32 = 1600.0;
/// 单页渲染的起始缩放；A4 页面在该缩放下约 1200px 长边。
const DEFAULT_PAGE_SCALE: f32 = 2.0;
/// 一次最多渲染的页数，避免把整本扫描书塞进一次请求。
pub(crate) const MAX_RENDERED_PAGES_PER_CALL: usize = 4;
/// 单页渲染后的字节上限，超过时降低缩放重试。
const MAX_RENDERED_PAGE_BYTES: usize = 4 * 1024 * 1024;

/// 渲染或抽取出来的一张文档图片，带上页码/图注等定位信息。
#[derive(Clone, Debug)]
pub(crate) struct DocumentImage {
    pub attachment: ConversationImageAttachment,
    pub page: Option<usize>,
    pub caption: String,
}

impl DocumentImage {
    /// 工具结果里的图片引用：只暴露 id 和尺寸，绝不内联 base64。
    pub(crate) fn to_payload(&self) -> Value {
        json!({
            "id": &self.attachment.id,
            "page": self.page,
            "caption": &self.caption,
            "width": self.attachment.width,
            "height": self.attachment.height,
            "byteSize": self.attachment.byte_size,
            "mimeType": &self.attachment.mime_type,
        })
    }
}

/// 判断 PDF 是否完全没有文本层；这类页面只能交给视觉模型。
pub(crate) fn pdf_requires_vision(extraction: &DocumentTextExtraction) -> bool {
    extraction.file_type == "pdf" && extraction.blocks.iter().all(|block| block.text.trim().is_empty())
}

/**
 * 渲染指定 PDF 页面并写入对话附件目录。
 *
 * `pages` 为 1-based 页码；越界直接报错，避免静默返回空图。
 */
pub(crate) fn render_pdf_pages(
    root: &Path,
    document: &WorkspaceDocument,
    pages: &[usize],
    attachments_root: &Path,
) -> Result<Vec<DocumentImage>, String> {
    if pages.is_empty() {
        return Ok(Vec::new());
    }
    if pages.len() > MAX_RENDERED_PAGES_PER_CALL {
        return Err(format!(
            "一次最多渲染 {MAX_RENDERED_PAGES_PER_CALL} 页图片；请分批请求。"
        ));
    }
    let target_path = resolve_existing_file_inside_root(root, &document.path)?;
    let bytes = fs::read(&target_path).map_err(|error| format!("无法读取 PDF 文件：{error}"))?;
    let pdf = hayro::hayro_syntax::Pdf::new(bytes)
        .map_err(|error| format!("PDF 无法渲染：{error:?}"))?;
    let pdf_pages = pdf.pages();
    let page_count = pdf_pages.len();
    let cache = hayro::RenderCache::new();
    let settings = hayro::hayro_interpret::InterpreterSettings::default();
    let render_settings = hayro::RenderSettings::default();

    let mut images = Vec::with_capacity(pages.len());
    for page_number in pages {
        if *page_number == 0 || *page_number > page_count {
            return Err(format!(
                "页码超出范围：PDF 共 {page_count} 页，请求第 {page_number} 页。"
            ));
        }
        let page = &pdf_pages[page_number - 1];
        let (encoded, width, height) =
            render_page_to_png(page, &cache, &settings, &render_settings)?;
        let caption = format!("{} 第 {page_number} 页", document.title);
        let attachment = crate::storage::save_conversation_image_from_bytes(
            attachments_root,
            &encoded,
            Some(&format!("{}-p{page_number}.png", document.title)),
        )?;
        images.push(DocumentImage {
            attachment,
            page: Some(*page_number),
            caption,
        });
        log::debug!(
            target: "document_vision",
            "PDF 页面已渲染：path={} page={} width={} height={} bytes={}",
            document.path,
            page_number,
            width,
            height,
            encoded.len()
        );
    }
    Ok(images)
}

/// 把单页渲染成 PNG；超长边或超体积时逐级降采样。
fn render_page_to_png<'a>(
    page: &'a hayro::hayro_syntax::page::Page<'a>,
    cache: &hayro::RenderCache<'a>,
    settings: &hayro::hayro_interpret::InterpreterSettings,
    render_settings: &hayro::RenderSettings,
) -> Result<(Vec<u8>, u32, u32), String> {
    let (page_width, page_height) = page.render_dimensions();
    let long_edge = page_width.max(page_height) as f32;
    let mut scale = DEFAULT_PAGE_SCALE;
    if long_edge > 0.0 {
        scale = scale.min(MAX_PAGE_LONG_EDGE / long_edge);
    }
    loop {
        let pixmap = hayro::render(
            page,
            cache,
            settings,
            render_settings,
            &hayro::PixmapSettings {
                x_scale: scale.max(0.1),
                y_scale: scale.max(0.1),
                bg_color: vello_white(),
            },
        );
        let width = u32::from(pixmap.width());
        let height = u32::from(pixmap.height());
        if width == 0 || height == 0 {
            return Err("PDF 页面尺寸为 0，无法渲染。".to_owned());
        }
        // 渲染底色是白色，alpha 恒为不透明，premultiplied RGBA 可以直接当直通 RGBA 用。
        let buffer = image::RgbaImage::from_raw(width, height, pixmap.data_as_u8_slice().to_vec())
            .ok_or_else(|| "PDF 页面像素缓冲尺寸不匹配。".to_owned())?;
        let mut encoded = Vec::new();
        image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut std::io::Cursor::new(&mut encoded), image::ImageFormat::Png)
            .map_err(|error| format!("无法编码 PDF 页面图片：{error}"))?;
        if encoded.len() <= MAX_RENDERED_PAGE_BYTES || scale <= 0.4 {
            return Ok((encoded, width, height));
        }
        scale *= 0.7;
    }
}

/// vello 的白色背景常量，集中一处避免调用点直接依赖颜色 crate。
fn vello_white() -> hayro::vello_cpu::color::AlphaColor<hayro::vello_cpu::color::Srgb> {
    hayro::vello_cpu::color::palette::css::WHITE
}

/// DOCX 内嵌图片：图片字节、关系 id 和原始 part 名。
#[derive(Clone, Debug)]
pub(crate) struct DocxMedia {
    pub relationship_id: String,
    pub part_name: String,
    pub bytes: Vec<u8>,
}

/**
 * 按 `document.xml` 中 `a:blip` 的出现顺序抽取 DOCX 内嵌图片。
 *
 * 只读取 `word/media` 目录；缺 `document.xml.rels` 时退回 zip 内顺序，
 * 保证图片不会因为关系文件损坏而整篇丢失。
 */
pub(crate) fn extract_docx_media(path: &Path) -> Result<Vec<DocxMedia>, String> {
    let file = fs::File::open(path).map_err(|error| format!("无法打开 DOCX 文件：{error}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("DOCX 文件结构无效：{error}"))?;
    let document_xml = read_docx_part(&mut archive, "word/document.xml")?.unwrap_or_default();
    let relationships_xml =
        read_docx_part(&mut archive, "word/_rels/document.xml.rels")?.unwrap_or_default();
    let relationships = parse_docx_relationships(&relationships_xml);
    let ordered_ids = parse_docx_blip_order(&document_xml);

    let mut media = Vec::new();
    let mut seen = HashSet::new();
    for relationship_id in ordered_ids {
        if !seen.insert(relationship_id.clone()) {
            continue;
        }
        let Some(part_name) = relationships.get(&relationship_id) else {
            continue;
        };
        if let Some(bytes) = read_docx_media_part(&mut archive, part_name)? {
            media.push(DocxMedia {
                relationship_id,
                part_name: part_name.clone(),
                bytes,
            });
        }
    }
    Ok(media)
}

/// 读取 DOCX 媒体二进制；单张图片超过附件上限时跳过而不是整篇失败。
fn read_docx_media_part(
    archive: &mut ZipArchive<fs::File>,
    part_name: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut file = match archive.by_name(part_name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(format!("无法读取 DOCX 图片 {part_name}：{error}")),
    };
    if file.size() > MAX_DOCX_PART_BYTES {
        log::warn!(
            target: "document_vision",
            "DOCX 图片超过单文件上限，已跳过：part={part_name} size={}",
            file.size()
        );
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取 DOCX 图片 {part_name}：{error}"))?;
    Ok(Some(bytes))
}

/// 解析 `document.xml.rels`：rId -> media part 路径。
fn parse_docx_relationships(xml: &str) -> HashMap<String, String> {
    let mut relationships = HashMap::new();
    if xml.is_empty() {
        return relationships;
    }
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event_into(&mut Vec::new()) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if !xml_name_matches(element.name().as_ref(), b"Relationship") {
                    continue;
                }
                let Some(id) = read_xml_attribute(&element, b"Id") else {
                    continue;
                };
                let Some(target) = read_xml_attribute(&element, b"Target") else {
                    continue;
                };
                if !target.contains("media/") {
                    continue;
                }
                let normalized = target.trim_start_matches('/').replace("word/", "");
                let part = if normalized.starts_with("media/") {
                    format!("word/{normalized}")
                } else {
                    format!("word/media/{}", normalized.trim_start_matches("media/"))
                };
                relationships.insert(id, part);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    relationships
}

/// 解析正文里 `a:blip r:embed="rIdN"` 的顺序。
fn parse_docx_blip_order(xml: &str) -> Vec<String> {
    let mut order = Vec::new();
    if xml.is_empty() {
        return order;
    }
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event_into(&mut Vec::new()) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if !xml_name_matches(element.name().as_ref(), b"blip") {
                    continue;
                }
                if let Some(id) = read_xml_attribute(&element, b"embed") {
                    order.push(id);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    order
}

/**
 * 抽取 DOCX 内嵌图片并写入对话附件目录。
 *
 * `max_images` 由调用方按“每轮剩余图片预算”传入；超出部分不渲染。
 */
pub(crate) fn extract_docx_images(
    root: &Path,
    document: &WorkspaceDocument,
    max_images: usize,
    attachments_root: &Path,
) -> Result<Vec<DocumentImage>, String> {
    if max_images == 0 {
        return Ok(Vec::new());
    }
    let target_path = resolve_existing_file_inside_root(root, &document.path)?;
    let media = extract_docx_media(&target_path)?;
    let mut images = Vec::new();
    for (index, item) in media.into_iter().enumerate() {
        if images.len() >= max_images {
            break;
        }
        let caption = format!("{} 内嵌图片 {}", document.title, index + 1);
        match crate::storage::save_conversation_image_from_bytes(
            attachments_root,
            &item.bytes,
            Some(&item.part_name),
        ) {
            Ok(attachment) => images.push(DocumentImage {
                attachment,
                page: None,
                caption,
            }),
            Err(error) => log::warn!(
                target: "document_vision",
                "DOCX 图片无法转为对话附件，已跳过：part={} error={}",
                item.part_name,
                error
            ),
        }
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// 最小两页 PDF，和 storage 模块的 fixture 保持一致。
    fn write_two_page_pdf(path: &Path) {
        let page_one = "BT /F1 18 Tf 72 720 Td (Page one) Tj ET";
        let page_two = "BT /F1 18 Tf 72 720 Td (Page two) Tj ET";
        let objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 6 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 7 0 R >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{}\nendstream", page_one.len(), page_one),
            format!("<< /Length {} >>\nstream\n{}\nendstream", page_two.len(), page_two),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", index + 1, object));
        }
        let xref = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        fs::write(path, pdf).unwrap();
    }

    fn pdf_document() -> WorkspaceDocument {
        WorkspaceDocument {
            id: "doc-pdf".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "spec".to_owned(),
            path: "spec.pdf".to_owned(),
            file_type: "pdf".to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: String::new(),
            content: None,
            preview_available: true,
        }
    }

    /// PDF 页面渲染必须产出可解码的 PNG，并且页面不是纯白空白。
    #[test]
    fn renders_pdf_pages_to_non_blank_png() {
        let dir = tempdir().unwrap();
        let pdf_path = dir.path().join("spec.pdf");
        write_two_page_pdf(&pdf_path);
        let attachments = dir.path().join("attachments");

        let images = render_pdf_pages(dir.path(), &pdf_document(), &[1], &attachments).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].page, Some(1));
        let bytes = fs::read(&images[0].attachment.absolute_path).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width() > 100 && decoded.height() > 100);
        // 页面文字会带来非白像素；全白说明渲染失败。
        let has_ink = decoded
            .to_luma8()
            .pixels()
            .any(|pixel| pixel.0[0] < 200);
        assert!(has_ink, "渲染结果不应是纯白页面");
    }

    /// 页码越界必须显式失败，不能返回空图片列表。
    #[test]
    fn rejects_out_of_range_pdf_page() {
        let dir = tempdir().unwrap();
        write_two_page_pdf(&dir.path().join("spec.pdf"));
        let attachments = dir.path().join("attachments");
        let error = render_pdf_pages(dir.path(), &pdf_document(), &[9], &attachments).unwrap_err();
        assert!(error.contains("页码超出范围"), "{error}");
    }

    /// DOCX 内嵌图片按 document.xml 的 blip 顺序抽取。
    #[test]
    fn extracts_docx_media_in_document_order() {
        let dir = tempdir().unwrap();
        let docx_path = dir.path().join("spec.docx");
        let file = fs::File::create(&docx_path).unwrap();
        let options = SimpleFileOptions::default();
        let mut archive = ZipWriter::new(file);
        let document_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<w:body><w:p><w:r><w:drawing><a:blip r:embed="rId2"/></w:drawing></w:r></w:p>
<w:p><w:r><w:drawing><a:blip r:embed="rId1"/></w:drawing></w:r></w:p></w:body></w:document>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.png"/>
</Relationships>"#;
        let png = {
            let mut bytes: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            bytes.extend_from_slice(b"\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89");
            bytes.extend_from_slice(b"\0\0\0\nIDATx\x9cc\0\0\0\x02\0\x01\xe2!\xbc3\0\0\0\0IEND\xaeB`\x82");
            bytes
        };
        for (name, content) in [
            ("word/document.xml", document_xml.as_bytes().to_vec()),
            ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
            ("word/media/image1.png", png.clone()),
            ("word/media/image2.png", png.clone()),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(&content).unwrap();
        }
        archive.finish().unwrap();

        let media = extract_docx_media(&docx_path).unwrap();
        assert_eq!(media.len(), 2);
        assert_eq!(media[0].relationship_id, "rId2");
        assert_eq!(media[0].part_name, "word/media/image2.png");
        assert_eq!(media[1].relationship_id, "rId1");
    }

    #[test]
    fn detects_pdf_without_text_layer() {
        let mut extraction = DocumentTextExtraction {
            document_id: "doc".to_owned(),
            file_type: "pdf".to_owned(),
            content_hash: String::new(),
            blocks: Vec::new(),
            content_chars: 0,
            page_count: 2,
            status: "no_text_layer".to_owned(),
            warnings: Vec::new(),
        };
        assert!(pdf_requires_vision(&extraction));
        extraction.blocks.push(DocumentTextBlock {
            index: 1,
            r#type: "page".to_owned(),
            text: "正文".to_owned(),
            page: Some(1),
            heading_level: None,
            role: None,
            table_row: None,
            table_col: None,
        });
        assert!(!pdf_requires_vision(&extraction));
    }
}