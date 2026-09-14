use super::inbound::ImInboundImage;
use crate::domain::ConversationImageAttachment;
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::storage::{self, admit_conversation_image_bytes};
use reqwest::header::CONTENT_TYPE;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;

/** 个人微信 iLink CDN；入站图要用 encrypt_query_param 从这里拉密文，不能只 GET full_url。 */
const WEIXIN_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

/** IM 入站图片下载鉴权；平台专属 header 只在这里组装，不进入 JSONL 契约。 */
pub(crate) enum ImageFetchAuth {
    Public,
    Qq {
        token: String,
    },
    Feishu {
        token: String,
        domain: String,
    },
    Weixin {
        token: String,
        base_url: String,
    },
}

/** 下载并准入 sidecar 给出的图片引用；单张失败就跳过，不中断文字回合。 */
pub(crate) async fn fetch_and_admit_inbound_images(
    app: &AppHandle,
    provider_id: &str,
    auth: &ImageFetchAuth,
    message_id: &str,
    images: &[ImInboundImage],
) -> Vec<ConversationImageAttachment> {
    if images.is_empty() {
        return Vec::new();
    }

    let mut fetched = Vec::new();
    for (index, image) in images.iter().enumerate() {
        match fetch_inbound_image(auth, message_id, image).await {
            Ok(bytes) => {
                let name = image.name.trim();
                fetched.push((
                    bytes,
                    if name.is_empty() {
                        None
                    } else {
                        Some(name.to_owned())
                    },
                ));
            }
            Err(error) => {
                logging::write_app_event_best_effort(
                    app,
                    AppEventBuilder::new(
                        AppLogLevel::Warn,
                        AppLogCategory::Im,
                        "im_inbound_image_skipped",
                        "skipped",
                        logging::sanitize_log_text(&error),
                    )
                    .metadata(json!({
                        "providerId": provider_id,
                        "imageIndex": index,
                        "hasUrl": is_http_url(&image.url),
                        "hasResourceId": !image.resource_id.trim().is_empty(),
                        "hasEncryptQuery": !image.encrypt_query.trim().is_empty(),
                        "hasAesKey": !image.aes_key.trim().is_empty(),
                        "hasBytes": !image.bytes_base64.trim().is_empty(),
                        "hasMessageId": !message_id.trim().is_empty(),
                        "claimedMime": !image.mime_type.trim().is_empty(),
                    })),
                );
            }
        }
    }

    let Ok(root) = storage::conversation_attachments_root(app) else {
        return Vec::new();
    };
    let admitted = admit_conversation_image_bytes(&root, &fetched);
    if admitted.len() != fetched.len() {
        logging::write_app_event_best_effort(
            app,
            AppEventBuilder::new(
                AppLogLevel::Warn,
                AppLogCategory::Im,
                "im_inbound_image_skipped",
                "skipped",
                "部分入站图片未通过格式或大小校验。",
            )
            .metadata(json!({
                "providerId": provider_id,
                "fetchedCount": fetched.len(),
                "admittedCount": admitted.len(),
            })),
        );
    }
    admitted
}

async fn fetch_inbound_image(
    auth: &ImageFetchAuth,
    message_id: &str,
    image: &ImInboundImage,
) -> Result<Vec<u8>, String> {
    if !image.bytes_base64.trim().is_empty() {
        let bytes = decode_base64_image(&image.bytes_base64)?;
        return maybe_decrypt_weixin_image(bytes, &image.aes_key);
    }

    match auth {
        ImageFetchAuth::Weixin { token, base_url } => {
            fetch_weixin_image(token, base_url, image).await
        }
        ImageFetchAuth::Feishu { token, domain } if !image.resource_id.trim().is_empty() => {
            download_feishu_resource(domain, token, message_id, image.resource_id.trim()).await
        }
        _ if is_http_url(&image.url) => {
            download_http_url(
                &image.url,
                authorization_header(auth),
                extra_headers(auth),
            )
            .await
        }
        _ => Err("缺少可下载的图片地址。".to_owned()),
    }
}

/** 个人微信：优先 CDN encrypt_query_param，再尝试 full_url，最后才打 getmedia。 */
async fn fetch_weixin_image(
    token: &str,
    base_url: &str,
    image: &ImInboundImage,
) -> Result<Vec<u8>, String> {
    let mut last_error = "缺少可下载的图片地址。".to_owned();

    if !image.encrypt_query.trim().is_empty() {
        match download_weixin_cdn(image.encrypt_query.trim()).await {
            Ok(bytes) => return maybe_decrypt_weixin_image(bytes, &image.aes_key),
            Err(error) => last_error = error,
        }
    }

    if is_http_url(&image.url) {
        match download_http_url(&image.url, None, Vec::new()).await {
            Ok(bytes) => match maybe_decrypt_weixin_image(bytes, &image.aes_key) {
                Ok(plain) => return Ok(plain),
                Err(error) => last_error = error,
            },
            Err(error) => last_error = error,
        }
    }

    if !image.resource_id.trim().is_empty() || !image.encrypt_query.trim().is_empty() {
        match download_weixin_resource(base_url, token, image).await {
            Ok(bytes) => return maybe_decrypt_weixin_image(bytes, &image.aes_key),
            Err(error) => last_error = error,
        }
    }

    Err(last_error)
}

fn authorization_header(auth: &ImageFetchAuth) -> Option<String> {
    match auth {
        ImageFetchAuth::Qq { token } if !token.trim().is_empty() => {
            Some(format!("QQBot {}", token.trim()))
        }
        ImageFetchAuth::Feishu { token, .. } if !token.trim().is_empty() => {
            Some(format!("Bearer {}", token.trim()))
        }
        ImageFetchAuth::Weixin { token, .. } if !token.trim().is_empty() => {
            Some(format!("Bearer {}", token.trim()))
        }
        _ => None,
    }
}

fn extra_headers(auth: &ImageFetchAuth) -> Vec<(String, String)> {
    match auth {
        ImageFetchAuth::Weixin { .. } => vec![
            (
                "AuthorizationType".to_owned(),
                "ilink_bot_token".to_owned(),
            ),
            ("X-WECHAT-UIN".to_owned(), random_wechat_uin()),
        ],
        _ => Vec::new(),
    }
}

async fn download_http_url(
    url: &str,
    authorization: Option<String>,
    extra_headers: Vec<(String, String)>,
) -> Result<Vec<u8>, String> {
    if !is_http_url(url) {
        return Err("图片地址无效。".to_owned());
    }
    let client = http_client()?;
    let mut request = client.get(url);
    if let Some(authorization) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, authorization);
    }
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("无法下载图片：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("下载图片失败：HTTP {}", response.status()));
    }
    read_limited_bytes(response).await
}

async fn download_weixin_cdn(encrypt_query: &str) -> Result<Vec<u8>, String> {
    download_http_url(&weixin_cdn_download_url(encrypt_query), None, Vec::new()).await
}

fn weixin_cdn_download_url(encrypt_query: &str) -> String {
    format!(
        "{WEIXIN_CDN_BASE_URL}/download?encrypted_query_param={}",
        percent_encode_query(encrypt_query)
    )
}

/** 与 Python urllib.parse.quote(safe="") 对齐：字母数字和 -_. 外全部百分号编码。 */
fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

async fn download_feishu_resource(
    domain: &str,
    token: &str,
    message_id: &str,
    resource_id: &str,
) -> Result<Vec<u8>, String> {
    if message_id.trim().is_empty() {
        return Err("飞书图片缺少 message_id。".to_owned());
    }
    let base = if domain == "lark" {
        "https://open.larksuite.com"
    } else {
        "https://open.feishu.cn"
    };
    let client = http_client()?;
    let mut last_error = "无法下载飞书图片。".to_owned();
    let resource_types = if resource_id.starts_with("img_") {
        ["image", "file"]
    } else {
        ["file", "image"]
    };
    for resource_type in resource_types {
        let url = feishu_resource_url(base, message_id, resource_id, resource_type)?;
        let response = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|error| format!("无法下载飞书图片：{error}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            last_error = format_feishu_download_error(status, &body);
            continue;
        }
        let content_type = response_content_type(&response);
        let bytes = read_limited_bytes(response).await?;
        if content_type.contains("json") || bytes.starts_with(b"{") {
            last_error = format_feishu_download_error(
                reqwest::StatusCode::OK,
                std::str::from_utf8(&bytes).unwrap_or_default(),
            );
            continue;
        }
        return Ok(bytes);
    }
    Err(last_error)
}

fn feishu_resource_url(
    base: &str,
    message_id: &str,
    resource_id: &str,
    resource_type: &str,
) -> Result<String, String> {
    let mut url =
        reqwest::Url::parse(base).map_err(|_| "飞书域名无效。".to_owned())?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "飞书域名无效。".to_owned())?;
        segments.extend([
            "open-apis",
            "im",
            "v1",
            "messages",
            message_id.trim(),
            "resources",
            resource_id.trim(),
        ]);
    }
    url.query_pairs_mut().append_pair("type", resource_type);
    Ok(url.to_string())
}

fn format_feishu_download_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
        let msg = value.get("msg").and_then(Value::as_str).unwrap_or_default();
        return format!(
            "下载飞书图片失败：HTTP {status} code={code} {}",
            logging::sanitize_log_text(msg)
        );
    }
    format!("下载飞书图片失败：HTTP {status}")
}

async fn download_weixin_resource(
    base_url: &str,
    token: &str,
    image: &ImInboundImage,
) -> Result<Vec<u8>, String> {
    let base = base_url.trim_end_matches('/');
    let client = http_client()?;
    let payload = json!({
        "base_info": { "channel_version": "1.0.0" },
        "fileid": image.resource_id,
        "aeskey": image.aes_key,
        "encrypt_query": image.encrypt_query,
        "media_id": image.resource_id,
    });
    let mut last_error = "无法下载微信图片。".to_owned();
    for path in [
        "/ilink/bot/getmedia",
        "/ilink/bot/get_media",
        "/ilink/bot/get_file",
        "/ilink/bot/get_msg_image",
        "/ilink/bot/getcdn",
    ] {
        let response = client
            .post(format!("{base}{path}"))
            .header(CONTENT_TYPE, "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .header("X-WECHAT-UIN", random_wechat_uin())
            .json(&payload)
            .send()
            .await;
        let Ok(response) = response else {
            continue;
        };
        if !response.status().is_success() {
            last_error = format!("下载微信图片失败：HTTP {}", response.status());
            continue;
        }
        match read_weixin_image_response(response).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

async fn read_weixin_image_response(response: reqwest::Response) -> Result<Vec<u8>, String> {
    let content_type = response_content_type(&response);
    let bytes = read_limited_bytes(response).await?;
    if content_type.contains("json") || bytes.starts_with(b"{") {
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| "微信图片响应无效。".to_owned())?;
        if let Some(url) = first_json_url(&value) {
            return download_http_url(url, None, Vec::new()).await;
        }
        if let Some(encoded) = first_json_base64(&value) {
            return decode_base64_image(encoded);
        }
        return Err("微信图片响应缺少文件。".to_owned());
    }
    Ok(bytes)
}

async fn read_limited_bytes(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if let Some(length) = response.content_length() {
        if length as usize > storage::MAX_SINGLE_PASTE_IMAGE_BYTES {
            return Err("单张图片超过 20MB，已忽略。".to_owned());
        }
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("无法读取图片内容：{error}"))?;
    if bytes.is_empty() {
        return Err("图片内容为空。".to_owned());
    }
    if bytes.len() > storage::MAX_SINGLE_PASTE_IMAGE_BYTES {
        return Err("单张图片超过 20MB，已忽略。".to_owned());
    }
    Ok(bytes.to_vec())
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| format!("无法创建图片下载 client：{error}"))
}

fn response_content_type(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn first_json_url(value: &Value) -> Option<&str> {
    for key in ["url", "full_url", "file_url", "cdn_url", "pic_url", "picurl"] {
        if let Some(url) = value.get(key).and_then(Value::as_str).filter(|item| is_http_url(item))
        {
            return Some(url);
        }
    }
    None
}

fn first_json_base64(value: &Value) -> Option<&str> {
    for key in ["data", "file_data", "buffer", "content"] {
        if let Some(encoded) = value.get(key).and_then(Value::as_str).filter(|item| !item.is_empty())
        {
            return Some(encoded);
        }
    }
    None
}

fn decode_base64_image(encoded: &str) -> Result<Vec<u8>, String> {
    decode_base64_padded(
        encoded
            .split_once(',')
            .map(|(_, rest)| rest)
            .unwrap_or(encoded)
            .trim(),
    )
}

fn decode_base64_padded(value: &str) -> Result<Vec<u8>, String> {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};
    use base64::Engine as _;
    let mut padded = value.trim().to_owned();
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    STANDARD
        .decode(&padded)
        .or_else(|_| URL_SAFE.decode(&padded))
        .map_err(|_| "微信图片内容不是有效的 base64。".to_owned())
}

fn maybe_decrypt_weixin_image(bytes: Vec<u8>, aeskey: &str) -> Result<Vec<u8>, String> {
    if looks_like_image(&bytes) {
        return Ok(bytes);
    }
    if aeskey.trim().is_empty() {
        return Err("下载结果不是可识别的图片。".to_owned());
    }
    let decrypted = decrypt_weixin_aes(&bytes, aeskey)?;
    if looks_like_image(&decrypted) {
        return Ok(decrypted);
    }
    Err("微信图片解密后仍无法识别格式。".to_owned())
}

fn decrypt_weixin_aes(bytes: &[u8], aeskey: &str) -> Result<Vec<u8>, String> {
    if bytes.is_empty() || bytes.len() % 16 != 0 {
        return Err("微信图片密文长度无效。".to_owned());
    }
    let keys = weixin_aes_key_candidates(aeskey);
    if keys.is_empty() {
        return Err("微信 aeskey 无效。".to_owned());
    }
    for key in &keys {
        if let Ok(plain) = decrypt_aes_ecb(key, bytes) {
            if looks_like_image(&plain) {
                return Ok(plain);
            }
        }
        if key.len() >= 16 {
            if let Ok(plain) = decrypt_aes_cbc(key, &key[..16], bytes) {
                if looks_like_image(&plain) {
                    return Ok(plain);
                }
            }
        }
        if bytes.len() > 16 {
            if let Ok(plain) = decrypt_aes_cbc(key, &bytes[..16], &bytes[16..]) {
                if looks_like_image(&plain) {
                    return Ok(plain);
                }
            }
        }
    }
    Err("无法使用 aeskey 解密微信图片。".to_owned())
}

/** 解析 iLink aeskey：hex、原始 16 字节的 base64、以及 hex 字符串再 base64。 */
fn weixin_aes_key_candidates(aeskey: &str) -> Vec<Vec<u8>> {
    let mut keys = Vec::new();
    let trimmed = aeskey.trim();
    if trimmed.is_empty() {
        return keys;
    }

    let mut push = |key: Vec<u8>| {
        if matches!(key.len(), 16 | 24 | 32) && !keys.iter().any(|existing| existing == &key) {
            keys.push(key);
        }
    };

    if trimmed.len() % 2 == 0 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        if let Ok(decoded) = decode_hex(trimmed) {
            push(decoded);
        }
    }

    if let Ok(raw) = decode_base64_padded(trimmed) {
        push(raw.clone());
        if let Ok(text) = std::str::from_utf8(&raw) {
            let text = text.trim();
            if text.len() % 2 == 0 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                if let Ok(decoded) = decode_hex(text) {
                    push(decoded);
                }
            }
        }
    }

    keys
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    let mut index = 0;
    while index + 1 < chars.len() {
        let hi = hex_nibble(chars[index])?;
        let lo = hex_nibble(chars[index + 1])?;
        bytes.push((hi << 4) | lo);
        index += 2;
    }
    Ok(bytes)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("微信 aeskey 不是有效十六进制。".to_owned()),
    }
}

fn decrypt_aes_ecb(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
    use aes::{Aes128, Aes192, Aes256};

    if data.is_empty() || data.len() % 16 != 0 {
        return Err("微信图片密文长度无效。".to_owned());
    }

    let mut out = data.to_vec();
    match key.len() {
        16 => {
            let cipher =
                Aes128::new_from_slice(key).map_err(|_| "微信 aeskey 无效。".to_owned())?;
            for chunk in out.chunks_mut(16) {
                cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
            }
        }
        24 => {
            let cipher =
                Aes192::new_from_slice(key).map_err(|_| "微信 aeskey 无效。".to_owned())?;
            for chunk in out.chunks_mut(16) {
                cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
            }
        }
        32 => {
            let cipher =
                Aes256::new_from_slice(key).map_err(|_| "微信 aeskey 无效。".to_owned())?;
            for chunk in out.chunks_mut(16) {
                cipher.decrypt_block(GenericArray::from_mut_slice(chunk));
            }
        }
        _ => return Err("微信 aeskey 长度不支持。".to_owned()),
    }
    Ok(pkcs7_unpad(&out))
}

fn pkcs7_unpad(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let pad_len = data[data.len() - 1] as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > data.len() {
        return data.to_vec();
    }
    if data[data.len() - pad_len..]
        .iter()
        .any(|&byte| byte as usize != pad_len)
    {
        return data.to_vec();
    }
    data[..data.len() - pad_len].to_vec()
}

fn decrypt_aes_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use aes::{Aes128, Aes192, Aes256};
    use cbc::Decryptor;

    if iv.len() != 16 || data.len() % 16 != 0 || data.is_empty() {
        return Err("微信图片密文长度无效。".to_owned());
    }
    let iv: [u8; 16] = iv.try_into().map_err(|_| "微信图片 IV 无效。".to_owned())?;
    match key.len() {
        16 => {
            let key: [u8; 16] = key.try_into().map_err(|_| "微信 aeskey 无效。".to_owned())?;
            Decryptor::<Aes128>::new(&key.into(), &iv.into())
                .decrypt_padded_vec_mut::<Pkcs7>(data)
                .map_err(|_| "微信图片 AES-128 解密失败。".to_owned())
        }
        24 => {
            let key: [u8; 24] = key.try_into().map_err(|_| "微信 aeskey 无效。".to_owned())?;
            Decryptor::<Aes192>::new(&key.into(), &iv.into())
                .decrypt_padded_vec_mut::<Pkcs7>(data)
                .map_err(|_| "微信图片 AES-192 解密失败。".to_owned())
        }
        32 => {
            let key: [u8; 32] = key.try_into().map_err(|_| "微信 aeskey 无效。".to_owned())?;
            Decryptor::<Aes256>::new(&key.into(), &iv.into())
                .decrypt_padded_vec_mut::<Pkcs7>(data)
                .map_err(|_| "微信图片 AES-256 解密失败。".to_owned())
        }
        _ => Err("微信 aeskey 长度不支持。".to_owned()),
    }
}

fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
        || (bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff)
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

fn is_http_url(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn random_wechat_uin() -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD.encode(uuid::Uuid::new_v4().as_u128().to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{
        decrypt_weixin_aes, feishu_resource_url, is_http_url, looks_like_image, pkcs7_unpad,
        weixin_aes_key_candidates, weixin_cdn_download_url,
    };

    #[test]
    fn http_url_rejects_non_network_schemes() {
        assert!(is_http_url("https://example.com/a.png"));
        assert!(is_http_url("http://example.com/a.png"));
        assert!(!is_http_url("file:///tmp/a.png"));
        assert!(!is_http_url("data:image/png;base64,abc"));
        assert!(!is_http_url(""));
    }

    #[test]
    fn builds_weixin_cdn_url_from_encrypt_query() {
        let url = weixin_cdn_download_url("abc+def/g=");
        assert!(url.starts_with(
            "https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param="
        ));
        assert!(url.contains("abc%2Bdef%2Fg%3D"));
    }

    #[test]
    fn builds_feishu_resource_url_with_encoded_path() {
        let url = feishu_resource_url(
            "https://open.feishu.cn",
            "om_1",
            "img_v2_a+b",
            "image",
        )
        .expect("url");
        assert!(url.contains("/open-apis/im/v1/messages/om_1/resources/"));
        assert!(url.contains("type=image"));
        assert!(url.contains("img_v2_a"));
        assert!(!url.contains(" "));
    }

    #[test]
    fn parses_weixin_aes_key_encodings() {
        let key = [0x11u8; 16];
        let hex = to_hex(&key);
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;
        let b64_raw = STANDARD.encode(key);
        let b64_hex = STANDARD.encode(hex.as_bytes());

        assert_eq!(weixin_aes_key_candidates(&hex)[0], key);
        assert!(weixin_aes_key_candidates(&b64_raw).iter().any(|item| item == &key));
        assert!(weixin_aes_key_candidates(&b64_hex).iter().any(|item| item == &key));
    }

    #[test]
    fn decrypts_weixin_aes_ecb_payload() {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        use aes::Aes128;
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;

        let png = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R',
        ];
        let key = [0x11u8; 16];
        let mut encrypted = pkcs7_pad_for_test(&png);
        let cipher = Aes128::new_from_slice(&key).expect("cipher");
        for chunk in encrypted.chunks_mut(16) {
            cipher.encrypt_block(GenericArray::from_mut_slice(chunk));
        }

        let from_hex = decrypt_weixin_aes(&encrypted, &to_hex(&key)).expect("hex");
        assert!(looks_like_image(&from_hex));
        assert_eq!(&from_hex[..png.len()], &png);

        let from_b64_hex =
            decrypt_weixin_aes(&encrypted, &STANDARD.encode(to_hex(&key).as_bytes())).expect("b64 hex");
        assert_eq!(&from_b64_hex[..png.len()], &png);

        let from_b64_raw = decrypt_weixin_aes(&encrypted, &STANDARD.encode(key)).expect("b64 raw");
        assert_eq!(&from_b64_raw[..png.len()], &png);
    }

    #[test]
    fn decrypts_weixin_aes_cbc_payload_as_fallback() {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use aes::Aes128;
        use cbc::Encryptor;

        let png = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R',
        ];
        let key = [0x22u8; 16];
        let encrypted = Encryptor::<Aes128>::new(&key.into(), &key.into())
            .encrypt_padded_vec_mut::<Pkcs7>(&png);
        let decrypted = decrypt_weixin_aes(&encrypted, &to_hex(&key)).expect("decrypt");
        assert!(looks_like_image(&decrypted));
        assert_eq!(&decrypted[..png.len()], &png);
        assert_eq!(pkcs7_unpad(&pkcs7_pad_for_test(&png)), png);
    }

    fn to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn pkcs7_pad_for_test(data: &[u8]) -> Vec<u8> {
        let pad_len = 16 - (data.len() % 16);
        let mut out = data.to_vec();
        out.extend(std::iter::repeat(pad_len as u8).take(pad_len));
        out
    }
}
