//! URL 语法、公网地址判断、重定向逐跳校验，以及 HTML 到 Markdown。

use std::net::IpAddr;

use scraper::{ElementRef, Html, Node};

use super::{FetchedPage, WebError};

/** 原始 URL 最长字节数。 */
pub(crate) const MAX_URL_BYTES: usize = 2048;

/** 最多跟随的重定向次数。 */
pub(crate) const MAX_REDIRECTS: usize = 3;

/** 响应体最多读取的字节数。 */
pub(crate) const MAX_RESPONSE_BYTES: usize = 1_048_576;

/** HTML 转换的最大嵌套深度。超过后拒绝，避免畸形标签拖住运行时。 */
pub(crate) const MAX_HTML_DEPTH: usize = 512;

const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "iframe", "object", "embed",
];

/** 一次 HTTP 交换的结果。重定向由抓页循环自己处理。 */
pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) location: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) body: Vec<u8>,
    pub(crate) truncated_by_bytes: bool,
}

/** 测试和真实网络共用的解析与请求接口。 */
pub(crate) trait HttpExchange {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, WebError>;
    fn exchange(&self, url: &str) -> Result<HttpResponse, WebError>;
}

/** 只检查语法：http(s)、无凭据、长度。不访问网络。 */
pub(crate) fn validate_url_syntax(input: &str) -> Result<reqwest::Url, WebError> {
    if input.len() > MAX_URL_BYTES {
        return Err(WebError::InvalidUrl);
    }
    let url = reqwest::Url::parse(input.trim()).map_err(|_| WebError::InvalidUrl)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(WebError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebError::BlockedUrl);
    }
    if url.host_str().is_none() {
        return Err(WebError::InvalidUrl);
    }
    Ok(url)
}

/** 私网、环回、链路本地、元数据地址和未指定地址都拒绝。 */
pub(crate) fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return ip_is_blocked(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            let unique_local = (segments[0] & 0xfe00) == 0xfc00;
            let link_local = (segments[0] & 0xffc0) == 0xfe80;
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || unique_local
                || link_local
        }
    }
}

fn assess_destination(url: &reqwest::Url, http: &dyn HttpExchange) -> Result<(), WebError> {
    let host = url.host_str().ok_or(WebError::InvalidUrl)?;
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return Err(WebError::BlockedUrl);
    }
    // IP 字面量先本地判断，避免为 127.0.0.1 这类地址做 DNS。
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if ip_is_blocked(ip) {
            Err(WebError::BlockedUrl)
        } else {
            Ok(())
        };
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addresses = http.resolve(host, port)?;
    if addresses.is_empty() || addresses.iter().any(|ip| ip_is_blocked(*ip)) {
        return Err(WebError::BlockedUrl);
    }
    Ok(())
}

fn join_redirect(current: &reqwest::Url, location: &str) -> Result<reqwest::Url, WebError> {
    let next = reqwest::Url::parse(location)
        .or_else(|_| current.join(location))
        .map_err(|_| WebError::InvalidUrl)?;
    validate_url_syntax(next.as_str())
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/** 逐跳校验后抓取正文，并在需要时把 HTML 转成 Markdown。 */
pub(crate) fn fetch_document(
    start: &str,
    http: &dyn HttpExchange,
) -> Result<FetchedPage, WebError> {
    let mut current = validate_url_syntax(start)?;
    let mut hops = 0usize;
    loop {
        assess_destination(&current, http)?;
        let response = http.exchange(current.as_str())?;
        if is_redirect(response.status) {
            if hops >= MAX_REDIRECTS {
                return Err(WebError::TooManyRedirects);
            }
            let location = response.location.ok_or(WebError::InvalidUrl)?;
            current = join_redirect(&current, &location)?;
            hops += 1;
            continue;
        }
        if !(200..300).contains(&response.status) {
            return Err(WebError::HttpStatus);
        }
        return decode_page(&current, response);
    }
}

fn decode_page(url: &reqwest::Url, response: HttpResponse) -> Result<FetchedPage, WebError> {
    let content_type = response.content_type.unwrap_or_default();
    let kind = classify_content_type(&content_type).ok_or(WebError::UnsupportedContent)?;
    let charset = charset_label(&content_type);
    if let Some(charset) = charset {
        if !matches!(charset.as_str(), "utf-8" | "utf8" | "us-ascii" | "ascii") {
            return Err(WebError::UnsupportedCharset);
        }
    }
    let raw = String::from_utf8_lossy(&response.body).into_owned();
    let (title, text) = match kind {
        PageKind::Html => {
            let title = html_title(&raw).unwrap_or_else(|| host_label(url));
            let markdown = html_to_markdown(&raw)?;
            (title, markdown)
        }
        PageKind::Text => (host_label(url), raw),
    };
    Ok(FetchedPage {
        final_url: url.to_string(),
        title,
        text,
        byte_truncated: response.truncated_by_bytes,
    })
}

enum PageKind {
    Html,
    Text,
}

fn classify_content_type(content_type: &str) -> Option<PageKind> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime.is_empty() {
        return None;
    }
    if mime == "text/html" || mime == "application/xhtml+xml" {
        return Some(PageKind::Html);
    }
    if mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/xml"
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
    {
        return Some(PageKind::Text);
    }
    None
}

fn charset_label(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let mut pieces = part.trim().splitn(2, '=');
        if pieces.next()?.trim().eq_ignore_ascii_case("charset") {
            return Some(pieces.next()?.trim().trim_matches('"').to_ascii_lowercase());
        }
    }
    None
}

fn host_label(url: &reqwest::Url) -> String {
    url.host_str().unwrap_or("网页").to_owned()
}

fn html_title(html: &str) -> Option<String> {
    let dom = Html::parse_document(html);
    let selector = scraper::Selector::parse("title").ok()?;
    let text = dom.select(&selector).next()?.text().collect::<String>();
    let trimmed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/** 把 HTML 收成 Markdown。script、样式和隐藏节点不进入正文。 */
pub(crate) fn html_to_markdown(input: &str) -> Result<String, WebError> {
    let dom = Html::parse_document(input);
    let mut markdown = render_element(dom.root_element(), 0)?;
    while markdown.contains("\n\n\n") {
        markdown = markdown.replace("\n\n\n", "\n\n");
    }
    Ok(markdown.trim().to_owned())
}

fn render_element(element: ElementRef<'_>, depth: usize) -> Result<String, WebError> {
    if depth > MAX_HTML_DEPTH {
        return Err(WebError::PageTooDeep);
    }
    let name = element.value().name();
    if SKIP_TAGS.contains(&name) || element_is_hidden(element) {
        return Ok(String::new());
    }
    match name {
        "h1" => Ok(format!("# {}\n\n", inline_text(element, depth)?)),
        "h2" => Ok(format!("## {}\n\n", inline_text(element, depth)?)),
        "h3" => Ok(format!("### {}\n\n", inline_text(element, depth)?)),
        "h4" | "h5" | "h6" => Ok(format!("#### {}\n\n", inline_text(element, depth)?)),
        "p" => Ok(format!("{}\n\n", inline_text(element, depth)?)),
        "li" => Ok(format!("- {}\n", inline_text(element, depth)?)),
        "pre" => Ok(format!("```\n{}\n```\n\n", inline_text(element, depth)?)),
        "br" => Ok("\n".to_owned()),
        "a" => {
            let label = inline_text(element, depth)?;
            match element.value().attr("href") {
                Some(href) if !href.trim().is_empty() && !label.is_empty() => {
                    Ok(format!("[{label}]({href})"))
                }
                _ => Ok(label),
            }
        }
        _ => render_children(element, depth),
    }
}

fn inline_text(element: ElementRef<'_>, depth: usize) -> Result<String, WebError> {
    let text = render_children(element, depth)?;
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn render_children(element: ElementRef<'_>, depth: usize) -> Result<String, WebError> {
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(_) => {
                if let Some(child_element) = ElementRef::wrap(child) {
                    out.push_str(&render_element(child_element, depth + 1)?);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn element_is_hidden(element: ElementRef<'_>) -> bool {
    let value = element.value();
    if value.attr("hidden").is_some() {
        return true;
    }
    if value
        .attr("aria-hidden")
        .is_some_and(|flag| flag.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    value.attr("style").is_some_and(|style| {
        let compact = style.replace(' ', "").to_ascii_lowercase();
        compact.contains("display:none") || compact.contains("visibility:hidden")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::Mutex;

    struct ScriptedHttp {
        dns: HashMap<String, Vec<IpAddr>>,
        responses: Mutex<HashMap<String, HttpResponse>>,
        requested: Mutex<Vec<String>>,
    }

    impl HttpExchange for ScriptedHttp {
        fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, WebError> {
            self.dns.get(host).cloned().ok_or(WebError::BlockedUrl)
        }

        fn exchange(&self, url: &str) -> Result<HttpResponse, WebError> {
            self.requested.lock().unwrap().push(url.to_owned());
            self.responses
                .lock()
                .unwrap()
                .remove(url)
                .ok_or(WebError::Network)
        }
    }

    fn public_dns() -> HashMap<String, Vec<IpAddr>> {
        HashMap::from([(
            "public.example".to_owned(),
            vec![IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))],
        )])
    }

    #[test]
    fn rejects_file_scheme_credentials_and_overlong_url() {
        assert!(validate_url_syntax("file:///etc/passwd").is_err());
        assert!(validate_url_syntax("https://user:secret@example.com/a").is_err());
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert!(validate_url_syntax(&long).is_err());
    }

    #[test]
    fn blocks_loopback_private_and_metadata_addresses() {
        assert!(ip_is_blocked(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(ip_is_blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(ip_is_blocked(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(ip_is_blocked(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!ip_is_blocked(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn rejects_literal_private_urls_without_fetching() {
        let http = ScriptedHttp {
            dns: HashMap::new(),
            responses: Mutex::new(HashMap::new()),
            requested: Mutex::new(Vec::new()),
        };
        assert!(fetch_document("http://127.0.0.1/secret", &http).is_err());
        assert!(fetch_document("http://10.0.0.8/secret", &http).is_err());
        assert!(fetch_document("http://169.254.169.254/latest", &http).is_err());
        assert!(http.requested.lock().unwrap().is_empty());
    }

    #[test]
    fn stops_when_a_redirect_targets_a_private_address() {
        let http = ScriptedHttp {
            dns: public_dns(),
            responses: Mutex::new(HashMap::from([(
                "https://public.example/start".to_owned(),
                HttpResponse {
                    status: 302,
                    location: Some("http://10.1.1.1/secret".to_owned()),
                    content_type: None,
                    body: b"secret".to_vec(),
                    truncated_by_bytes: false,
                },
            )])),
            requested: Mutex::new(Vec::new()),
        };
        let error = fetch_document("https://public.example/start", &http).unwrap_err();
        assert!(matches!(error, WebError::BlockedUrl));
        assert_eq!(
            http.requested.lock().unwrap().as_slice(),
            ["https://public.example/start"]
        );
    }

    #[test]
    fn html_omits_script_and_hidden_text() {
        let markdown = html_to_markdown(
            "<html><body><p>可见段落</p><script>secret_token</script><div hidden>隐藏</div></body></html>",
        )
        .unwrap();
        assert!(markdown.contains("可见段落"));
        assert!(!markdown.contains("secret_token"));
        assert!(!markdown.contains("隐藏"));
    }
}
