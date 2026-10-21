//! 浏览器 Agent 的安全策略：URL 白名单、操作分类、审批门禁。
//!
//! 这里只放纯函数，保证 URL 过滤、只读/非只读分类和审批判定可以
//! 在没有 Chromium 的环境下完整测试。

use serde::{Deserialize, Serialize};
use url::Url;

/** 浏览器功能设置；默认关闭，用户显式启用并确认下载后才可用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSettings {
    pub enabled: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self { enabled: false }
    }
}

/** 只允许普通网页；本地文件和浏览器内部页面不能交给模型。 */
pub fn normalize_web_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("URL 不能为空。".to_owned());
    }
    if trimmed.chars().count() > 4096 {
        return Err("URL 超过长度限制。".to_owned());
    }
    let parsed = Url::parse(trimmed)
        .or_else(|_| Url::parse(&format!("https://{trimmed}")))
        .map_err(|_| "URL 格式无效。".to_owned())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("只允许 http 或 https 页面。".to_owned());
    }
    if parsed.host_str().is_none() {
        return Err("URL 缺少主机名。".to_owned());
    }
    Ok(parsed.to_string())
}

/** 提取 URL origin 供审计日志使用；解析失败返回 None，不记录完整 URL。 */
pub fn url_origin(value: &str) -> Option<String> {
    Url::parse(value).ok().map(|url| url.origin().ascii_serialization())
}

/** 第一版搜索固定走 Bing；后续开放搜索引擎设置时再扩展。 */
pub fn bing_search_url(query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("搜索词不能为空。".to_owned());
    }
    if query.chars().count() > 512 {
        return Err("搜索词超过长度限制。".to_owned());
    }
    let mut url = Url::parse("https://www.bing.com/search").map_err(|_| "搜索地址无效。".to_owned())?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url.to_string())
}

/** 模型可见操作集合；读取是只读，其它默认要求审批或完全级。 */
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserOperationKind {
    Read,
    Mutate,
}

/** 列出全部已注册操作；工具 schema 描述与运行时分发共用同一份闭集。 */
pub const READ_OPERATIONS: &[&str] = &[
    "browser.get_state",
    "page.text.snapshot",
    "page.elements.snapshot",
    "page.wait_for",
];

pub const MUTATE_OPERATIONS: &[&str] = &[
    "browser.create_tab",
    "browser.activate_tab",
    "browser.switch_tab",
    "browser.release_tab",
    "browser.close_tab",
    "tab.navigate",
    "tab.search",
    "tab.reload",
    "tab.go_back",
    "tab.go_forward",
    "tab.stop_loading",
    "tab.wait_for_load",
    "page.element.click",
    "page.element.fill",
    "page.element.type_text",
    "page.element.press_key",
    "page.element.select_option",
    "page.element.set_checked",
    "page.element.scroll_into_view",
];

pub fn operation_kind(operation: &str) -> Option<BrowserOperationKind> {
    if READ_OPERATIONS.contains(&operation) {
        Some(BrowserOperationKind::Read)
    } else if MUTATE_OPERATIONS.contains(&operation) {
        Some(BrowserOperationKind::Mutate)
    } else {
        None
    }
}

/** 进阶级别下非只读操作等待用户确认；完全级自动执行。 */
pub fn operation_requires_approval(security_level: &str, operation: &str) -> bool {
    operation_kind(operation) == Some(BrowserOperationKind::Mutate) && security_level != "autonomous"
}

/** 浏览器功能只对本机交互会话的进阶/完全级别可见；IM、定时、规划模式一律拒绝。 */
pub fn session_allows_browser(
    has_im_identity: bool,
    has_schedule_identity: bool,
    plan_mode: bool,
    security_level: &str,
) -> bool {
    !has_im_identity
        && !has_schedule_identity
        && !plan_mode
        && matches!(security_level, "advanced" | "autonomous")
}

/** 文本快照单页最大字符数；超过后分页并返回 nextTextOffset。 */
pub const MAX_TEXT_SNAPSHOT_CHARS: usize = 6000;

/** 元素快照单次最多返回的元素数量，避免超大页面塞满模型上下文。 */
pub const MAX_SNAPSHOT_ELEMENTS: usize = 200;

/** 单个元素的文本/名称最大字符数。 */
pub const MAX_ELEMENT_TEXT_CHARS: usize = 240;

/**
 * 对页面正文做分页切片：offset 超界返回空页，truncated 标记是否还有剩余。
 * 按字符而不是字节切，保证中文等多字节文本不被截断出乱码。
 */
pub fn slice_text_page(text: &str, offset: usize, limit: usize) -> (String, bool, Option<usize>) {
    let limit = limit.max(1);
    let total = text.chars().count();
    if offset >= total {
        return (String::new(), false, None);
    }
    let sliced: String = text.chars().skip(offset).take(limit).collect();
    let end = offset + sliced.chars().count();
    let truncated = end < total;
    let next_offset = truncated.then_some(end);
    (sliced, truncated, next_offset)
}

/** 快照 ID：时间戳 + 随机尾巴，面板和模型都用它做失效判定。 */
#[allow(dead_code)]
pub fn new_snapshot_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("snap-{now}-{}", crate::storage::create_id("gen"))
}

/**
 * 快照失效判定：元素 reference 里带 snapshotId 前缀，当前快照 ID 或
 * 页面 generation 任一不匹配都算过期，要求模型重新快照。
 */
pub fn snapshot_reference_matches(reference: &str, snapshot_id: &str) -> bool {
    reference
        .strip_prefix(snapshot_id)
        .is_some_and(|rest| rest.starts_with('-'))
}

/** 审计条目的 approval 字段取值。 */
pub const APPROVAL_NOT_REQUIRED: &str = "not-required";
pub const APPROVAL_GRANTED: &str = "granted";
pub const APPROVAL_DENIED: &str = "denied";
pub const APPROVAL_TIMEOUT: &str = "timeout";
pub const APPROVAL_ABORTED: &str = "aborted";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_http_urls_and_rejects_local_files() {
        assert_eq!(
            normalize_web_url("example.com/path").unwrap(),
            "https://example.com/path"
        );
        assert_eq!(
            normalize_web_url("http://localhost:5173/").unwrap(),
            "http://localhost:5173/"
        );
        assert!(normalize_web_url("file:///etc/passwd").is_err());
        assert!(normalize_web_url("chrome://settings").is_err());
        assert!(normalize_web_url("javascript:alert(1)").is_err());
        assert!(normalize_web_url("").is_err());
    }

    #[test]
    fn url_origin_extracted_for_audit() {
        assert_eq!(
            url_origin("https://example.com/a/b?q=1").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(url_origin("not a url"), None);
    }

    #[test]
    fn bing_search_url_encodes_query() {
        let url = bing_search_url("rust CDP").unwrap();
        assert!(url.starts_with("https://www.bing.com/search?q="));
        assert!(url.contains("rust+CDP"));
        assert!(bing_search_url("   ").is_err());
    }

    #[test]
    fn session_gate_blocks_non_local_or_low_levels() {
        assert!(session_allows_browser(false, false, false, "advanced"));
        assert!(session_allows_browser(false, false, false, "autonomous"));
        assert!(!session_allows_browser(false, false, false, "basic"));
        assert!(!session_allows_browser(true, false, false, "advanced"));
        assert!(!session_allows_browser(false, true, false, "advanced"));
        assert!(!session_allows_browser(false, false, true, "advanced"));
        assert!(!session_allows_browser(false, false, true, "autonomous"));
    }

    #[test]
    fn text_paging_slices_by_chars_and_reports_next_offset() {
        let text = "一二三四五六七八九十";
        let (page, truncated, next) = slice_text_page(text, 0, 4);
        assert_eq!(page, "一二三四");
        assert!(truncated);
        assert_eq!(next, Some(4));
        let (tail, truncated, next) = slice_text_page(text, 8, 4);
        assert_eq!(tail, "九十");
        assert!(!truncated);
        assert_eq!(next, None);
        let (empty, truncated, next) = slice_text_page(text, 99, 4);
        assert!(empty.is_empty());
        assert!(!truncated);
        assert_eq!(next, None);
    }

    #[test]
    fn snapshot_reference_requires_prefix_and_dash() {
        assert!(snapshot_reference_matches("snap-1-gen-3", "snap-1-gen"));
        assert!(!snapshot_reference_matches("snap-1-gen-3", "snap-2-gen"));
        assert!(!snapshot_reference_matches("snap-1-gen", "snap-1-gen"));
    }
}
