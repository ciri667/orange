use super::*;
use crate::domain::{ConversationImageAttachment, NoteImageAttachmentInput};
use image::{
    codecs::jpeg::JpegEncoder, ColorType, DynamicImage, GenericImageView, ImageEncoder, ImageFormat,
};
use serde_json::{json, Value};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/** 对话附件相对 app_data 的目录，内容寻址后永不自动删除。 */
pub const CONVERSATION_ATTACHMENTS_DIR: &str = "attachments/v1";
/** 单条用户消息最多携带的对话图片数，和 @ 文件上限对齐。 */
pub const MAX_CONVERSATION_IMAGES_PER_MESSAGE: usize = 8;
/** 发给模型前的编码体积上限，避开常见 OpenAI-compatible 5MB 限制。 */
const MAX_REQUEST_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/** 发给模型前的长边上限，控制视觉 token。 */
const MAX_REQUEST_IMAGE_DIMENSION: u32 = 2048;
/** 历史装箱时每张图片按固定字符估算，避免把 base64 算进预算。 */
pub const ESTIMATED_IMAGE_CHARS: usize = 4800;
/** transcript 中的图片引用类型；HTTP 发送前再展开成 image_url。 */
pub const MODEL_IMAGE_REF_TYPE: &str = "orange_image";

const IMAGE_ID_LENGTH: usize = 64;
const STORED_EXTENSIONS: &[&str] = &["png", "jpg", "gif", "webp"];

/** 解析 app_data 下的对话附件根目录。 */
pub fn conversation_attachments_root(app: &AppHandle) -> Result<PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法解析应用数据目录：{error}"))?;
    Ok(app_data_dir.join(CONVERSATION_ATTACHMENTS_DIR))
}

/** 校验并写入一批对话图片；整批失败时不留下半成品。 */
pub fn save_conversation_images(
    root: &Path,
    images: &[NoteImageAttachmentInput],
) -> Result<Vec<ConversationImageAttachment>, String> {
    if images.is_empty() {
        return Err("没有可保存的图片。".to_owned());
    }
    if images.len() > MAX_CONVERSATION_IMAGES_PER_MESSAGE {
        return Err(format!(
            "单条消息最多上传 {MAX_CONVERSATION_IMAGES_PER_MESSAGE} 张图片。"
        ));
    }

    let prepared_images = prepare_image_attachments(images)?;
    fs::create_dir_all(root).map_err(|error| format!("无法创建对话附件目录：{error}"))?;

    let mut saved = Vec::with_capacity(prepared_images.len());
    for (index, prepared) in prepared_images.into_iter().enumerate() {
        let decoded = image::load_from_memory(&prepared.bytes)
            .map_err(|_| "无法解码对话图片，已阻止保存。".to_owned())?;
        let (width, height) = decoded.dimensions();
        let id = hash_bytes(&prepared.bytes);
        let absolute_path =
            write_content_addressed_image(root, &id, prepared.format.extension, &prepared.bytes)?;
        let name = images
            .get(index)
            .and_then(|image| image.original_file_name.as_deref())
            .map(sanitize_display_name)
            .filter(|value| !value.is_empty());

        saved.push(ConversationImageAttachment {
            id,
            mime_type: prepared.format.mime_type.to_owned(),
            byte_size: prepared.bytes.len(),
            width: Some(width),
            height: Some(height),
            name,
            absolute_path: absolute_path.to_string_lossy().to_string(),
        });
    }
    Ok(saved)
}

/** 从已下载字节准入一张对话图片；MIME 以文件头为准，不信任调用方声明。 */
pub fn save_conversation_image_from_bytes(
    root: &Path,
    bytes: &[u8],
    original_file_name: Option<&str>,
) -> Result<ConversationImageAttachment, String> {
    if bytes.is_empty() {
        return Err("图片内容为空，已阻止保存。".to_owned());
    }
    if bytes.len() > MAX_SINGLE_PASTE_IMAGE_BYTES {
        return Err("单张图片超过 20MB，已阻止保存。".to_owned());
    }
    let format = detect_image_attachment_format(bytes)?;
    let input = NoteImageAttachmentInput {
        mime_type: format.mime_type.to_owned(),
        bytes_base64: BASE64_STANDARD.encode(bytes),
        original_file_name: original_file_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    };
    let mut saved = save_conversation_images(root, &[input])?;
    saved.pop().ok_or_else(|| "保存对话图片失败。".to_owned())
}

/** 逐张准入已下载图片；单张失败或超限时跳过，不让坏图阻断整批文字。 */
pub fn admit_conversation_image_bytes(
    root: &Path,
    images: &[(Vec<u8>, Option<String>)],
) -> Vec<ConversationImageAttachment> {
    let mut saved = Vec::new();
    let mut total_bytes = 0usize;
    for (bytes, name) in images {
        if saved.len() >= MAX_CONVERSATION_IMAGES_PER_MESSAGE {
            break;
        }
        if total_bytes.saturating_add(bytes.len()) > MAX_PASTE_IMAGE_BATCH_BYTES {
            continue;
        }
        match save_conversation_image_from_bytes(root, bytes, name.as_deref()) {
            Ok(attachment) => {
                total_bytes = total_bytes.saturating_add(attachment.byte_size);
                saved.push(attachment);
            }
            Err(_) => continue,
        }
    }
    saved
}

/** 按 ID 重新解析附件；忽略调用方提供的路径，防止引用附件目录外的文件。 */
pub fn resolve_conversation_images(
    root: &Path,
    image_ids: &[String],
) -> Result<Vec<ConversationImageAttachment>, String> {
    if image_ids.len() > MAX_CONVERSATION_IMAGES_PER_MESSAGE {
        return Err(format!(
            "单条消息最多上传 {MAX_CONVERSATION_IMAGES_PER_MESSAGE} 张图片。"
        ));
    }

    let mut seen = HashSet::new();
    let mut resolved = Vec::with_capacity(image_ids.len());
    for image_id in image_ids {
        if !seen.insert(image_id.as_str()) {
            continue;
        }
        resolved.push(resolve_conversation_image(root, image_id)?);
    }
    Ok(resolved)
}

/** 读取一张已准入图片的元数据和原始字节。 */
pub fn resolve_conversation_image(
    root: &Path,
    image_id: &str,
) -> Result<ConversationImageAttachment, String> {
    let id = parse_image_id(image_id)?;
    let absolute_path = find_stored_image_path(root, id)?;
    let bytes = fs::read(&absolute_path).map_err(|_| "无法读取对话图片附件。".to_owned())?;
    if bytes.is_empty() {
        return Err("对话图片附件为空。".to_owned());
    }
    let format = detect_image_attachment_format(&bytes)?;
    let decoded =
        image::load_from_memory(&bytes).map_err(|_| "对话图片附件已损坏，无法解码。".to_owned())?;
    let (width, height) = decoded.dimensions();
    Ok(ConversationImageAttachment {
        id: id.to_owned(),
        mime_type: format.mime_type.to_owned(),
        byte_size: bytes.len(),
        width: Some(width),
        height: Some(height),
        name: None,
        absolute_path: absolute_path.to_string_lossy().to_string(),
    })
}

/** 把 transcript 里的 orange_image 展开成 OpenAI image_url；缺文件时降级为占位文本。 */
pub fn hydrate_model_images(root: &Path, messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| hydrate_model_message(root, message))
        .collect()
}

/** 是否需要在发送前展开图片引用。 */
pub fn messages_contain_image_refs(messages: &[Value]) -> bool {
    messages.iter().any(message_contains_image_ref)
}

fn message_contains_image_ref(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part.get("type").and_then(Value::as_str) == Some(MODEL_IMAGE_REF_TYPE))
        })
}

fn hydrate_model_message(root: &Path, message: &Value) -> Value {
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return message.clone();
    };
    if !parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some(MODEL_IMAGE_REF_TYPE))
    {
        return message.clone();
    }

    let hydrated_parts: Vec<Value> = parts
        .iter()
        .map(|part| {
            if part.get("type").and_then(Value::as_str) != Some(MODEL_IMAGE_REF_TYPE) {
                return part.clone();
            }
            let Some(image_id) = part.get("id").and_then(Value::as_str) else {
                return json!({ "type": "text", "text": "[图片无法读取]" });
            };
            match load_request_image(root, image_id) {
                Ok((bytes, mime_type)) => {
                    let encoded = BASE64_STANDARD.encode(bytes);
                    json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{mime_type};base64,{encoded}")
                        }
                    })
                }
                Err(_) => json!({ "type": "text", "text": "[图片无法读取]" }),
            }
        })
        .collect();

    let mut cloned = message.clone();
    cloned["content"] = json!(hydrated_parts);
    cloned
}

fn load_request_image(root: &Path, image_id: &str) -> Result<(Vec<u8>, &'static str), String> {
    let id = parse_image_id(image_id)?;
    let path = find_stored_image_path(root, id)?;
    let bytes = fs::read(&path).map_err(|_| "无法读取对话图片附件。".to_owned())?;
    project_request_image(&bytes)
}

/** 将原图压到模型请求预算内；小图且未超长边时保持原编码。 */
pub(crate) fn project_request_image(bytes: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let format = detect_image_attachment_format(bytes)?;
    let image = image::load_from_memory(bytes).map_err(|_| "无法解码对话图片。".to_owned())?;
    let (width, height) = image.dimensions();
    if bytes.len() <= MAX_REQUEST_IMAGE_BYTES && width.max(height) <= MAX_REQUEST_IMAGE_DIMENSION {
        return Ok((bytes.to_vec(), format.mime_type));
    }

    let mut current = fit_long_edge(image, MAX_REQUEST_IMAGE_DIMENSION);
    if let Some(encoded) = encode_within_limit(&current)? {
        return Ok(encoded);
    }

    let mut dimension = MAX_REQUEST_IMAGE_DIMENSION / 2;
    while dimension >= 32 {
        current = fit_long_edge(current, dimension);
        if let Some(encoded) = encode_within_limit(&current)? {
            return Ok(encoded);
        }
        dimension /= 2;
    }

    Err("图片过大，无法压缩到模型请求限制以内。".to_owned())
}

fn fit_long_edge(image: DynamicImage, max_dimension: u32) -> DynamicImage {
    let (width, height) = image.dimensions();
    let long_edge = width.max(height);
    if long_edge <= max_dimension {
        return image;
    }
    image.resize(
        max_dimension,
        max_dimension,
        image::imageops::FilterType::Triangle,
    )
}

fn encode_within_limit(image: &DynamicImage) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    if image.color().has_alpha() {
        let png = encode_png(image)?;
        if png.len() <= MAX_REQUEST_IMAGE_BYTES {
            return Ok(Some((png, "image/png")));
        }
    }

    for quality in [85, 70, 55, 40] {
        let jpeg = encode_jpeg(image, quality)?;
        if jpeg.len() <= MAX_REQUEST_IMAGE_BYTES {
            return Ok(Some((jpeg, "image/jpeg")));
        }
    }
    Ok(None)
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
        .map_err(|_| "无法编码 PNG 图片。".to_owned())?;
    Ok(encoded)
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = image.to_rgb8();
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, quality)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ColorType::Rgb8.into(),
        )
        .map_err(|_| "无法编码 JPEG 图片。".to_owned())?;
    Ok(encoded)
}

fn write_content_addressed_image(
    root: &Path,
    image_id: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let shard = &image_id[..2];
    let directory = root.join(shard);
    fs::create_dir_all(&directory).map_err(|error| format!("无法创建对话附件分片目录：{error}"))?;
    let target_path = directory.join(format!("{image_id}.{extension}"));
    if target_path.exists() {
        return Ok(target_path);
    }

    let mut temp_file = NamedTempFile::new_in(&directory)
        .map_err(|error| format!("无法创建对话附件临时文件：{error}"))?;
    temp_file
        .write_all(bytes)
        .map_err(|error| format!("无法写入对话附件：{error}"))?;
    temp_file
        .persist(&target_path)
        .map_err(|error| format!("无法保存对话附件：{}", error.error))?;
    Ok(target_path)
}

fn find_stored_image_path(root: &Path, image_id: &str) -> Result<PathBuf, String> {
    let shard = &image_id[..2];
    for extension in STORED_EXTENSIONS {
        let candidate = root.join(shard).join(format!("{image_id}.{extension}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("找不到对话图片附件。".to_owned())
}

fn parse_image_id(image_id: &str) -> Result<&str, String> {
    if image_id.len() == IMAGE_ID_LENGTH && image_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(image_id)
    } else {
        Err("图片附件引用无效。".to_owned())
    }
}

fn sanitize_display_name(value: &str) -> String {
    let leaf = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
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
    if leaf.is_empty() || leaf == "." || leaf == ".." {
        String::new()
    } else {
        leaf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 1x1 透明 PNG，供准入和引用解析测试使用。 */
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn png_input() -> NoteImageAttachmentInput {
        NoteImageAttachmentInput {
            mime_type: "image/png".to_owned(),
            bytes_base64: BASE64_STANDARD.encode(PNG_1X1),
            original_file_name: Some("C:\\Users\\demo\\shot.png".to_owned()),
        }
    }

    /** 准入后只保留内容哈希文件名，展示名去掉本地路径。 */
    #[test]
    fn save_conversation_images_is_content_addressed_and_strips_paths() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save_conversation_images(dir.path(), &[png_input()]).unwrap();

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id.len(), 64);
        assert_eq!(saved[0].mime_type, "image/png");
        assert_eq!(saved[0].width, Some(1));
        assert_eq!(saved[0].height, Some(1));
        assert_eq!(saved[0].name.as_deref(), Some("shot.png"));
        assert!(Path::new(&saved[0].absolute_path).is_file());

        let again = save_conversation_images(dir.path(), &[png_input()]).unwrap();
        assert_eq!(again[0].id, saved[0].id);
        assert_eq!(again[0].absolute_path, saved[0].absolute_path);
    }

    /** 伪造 MIME 或非法 ID 不得读到附件目录外的文件。 */
    #[test]
    fn resolve_conversation_images_rejects_invalid_ids_and_mime_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save_conversation_images(dir.path(), &[png_input()]).unwrap();

        let error = resolve_conversation_image(dir.path(), "../secret").unwrap_err();
        assert!(error.contains("无效"));

        let resolved = resolve_conversation_images(dir.path(), &[saved[0].id.clone()]).unwrap();
        assert_eq!(resolved[0].id, saved[0].id);
        assert_eq!(resolved[0].mime_type, "image/png");
    }

    /** SVG 和空批次必须在写入前被拒绝。 */
    #[test]
    fn save_conversation_images_rejects_unsupported_or_empty_batches() {
        let dir = tempfile::tempdir().unwrap();
        let empty = save_conversation_images(dir.path(), &[]).unwrap_err();
        assert!(empty.contains("没有可保存"));

        let svg = NoteImageAttachmentInput {
            mime_type: "image/svg+xml".to_owned(),
            bytes_base64: BASE64_STANDARD.encode(b"<svg></svg>"),
            original_file_name: None,
        };
        let rejected = save_conversation_images(dir.path(), &[svg]).unwrap_err();
        assert!(rejected.contains("仅支持"));
    }

    /** 模型请求展开必须使用 data URL，且不把附件路径写进 payload。 */
    #[test]
    fn hydrate_model_images_expands_refs_to_data_urls() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save_conversation_images(dir.path(), &[png_input()]).unwrap();
        let messages = vec![json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "看看这张图" },
                { "type": MODEL_IMAGE_REF_TYPE, "id": saved[0].id }
            ]
        })];

        let hydrated = hydrate_model_images(dir.path(), &messages);
        let url = hydrated[0]["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(!url.contains(&saved[0].absolute_path));
        assert!(messages_contain_image_refs(&messages));
        assert!(!messages_contain_image_refs(&hydrated));
    }

    /** 坏图必须跳过，合法图继续准入。 */
    #[test]
    fn admit_conversation_image_bytes_skips_invalid_and_keeps_valid() {
        let dir = tempfile::tempdir().unwrap();
        let admitted = admit_conversation_image_bytes(
            dir.path(),
            &[
                (b"<svg></svg>".to_vec(), Some("bad.svg".to_owned())),
                (PNG_1X1.to_vec(), Some("shot.png".to_owned())),
                (Vec::new(), None),
            ],
        );

        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].mime_type, "image/png");
        assert_eq!(admitted[0].name.as_deref(), Some("shot.png"));
    }

    /** 超长边原图会被压到请求预算内。 */
    #[test]
    fn project_request_image_resizes_oversized_png() {
        let oversized = DynamicImage::new_rgba8(2400, 800);
        let mut encoded = Vec::new();
        oversized
            .write_to(&mut Cursor::new(&mut encoded), ImageFormat::Png)
            .unwrap();

        let (projected, mime) = project_request_image(&encoded).unwrap();
        let decoded = image::load_from_memory(&projected).unwrap();
        assert!(decoded.dimensions().0.max(decoded.dimensions().1) <= MAX_REQUEST_IMAGE_DIMENSION);
        assert!(projected.len() <= MAX_REQUEST_IMAGE_BYTES);
        assert!(mime == "image/png" || mime == "image/jpeg");
    }
}
