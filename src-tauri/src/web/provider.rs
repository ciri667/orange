//! 联网搜索供应商抽象。宿主仍负责预算、缓存、引用和网页抓取。

use std::time::Duration;

use serde::Deserialize;

use super::{clip_chars, WebError, WebHit, MAX_WEB_SNIPPET_CHARS};

/** 搜索请求统一超时；供应商客户端不允许单独放大这个边界。 */
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

/** 设置页展示和 keyring 引用都依赖这份稳定清单。 */
pub(crate) struct WebSearchProviderDescriptor {
    pub(crate) id: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) key_reference: &'static str,
}

pub(crate) const WEB_SEARCH_PROVIDERS: &[WebSearchProviderDescriptor] = &[
    WebSearchProviderDescriptor {
        id: "tavily",
        display_name: "Tavily",
        key_reference: "orange-web-search-tavily",
    },
    WebSearchProviderDescriptor {
        id: "brave",
        display_name: "Brave Search",
        key_reference: "orange-web-search-brave",
    },
    WebSearchProviderDescriptor {
        id: "exa",
        display_name: "Exa",
        key_reference: "orange-web-search-exa",
    },
    WebSearchProviderDescriptor {
        id: "serper",
        display_name: "Serper",
        key_reference: "orange-web-search-serper",
    },
];

/** 所有内置搜索供应商都实现同一同步入口，调用方不需要理解各家响应格式。 */
pub(crate) trait WebSearchProvider: Send + Sync {
    fn search(&self, api_key: &str, query: &str, limit: usize) -> Result<Vec<WebHit>, WebError>;
}

/** 按 id 查找内置供应商；未知 id 只会在设置入口报错，不会静默回退。 */
pub(crate) fn find_provider(id: &str) -> Option<&'static dyn WebSearchProvider> {
    match id {
        "tavily" => Some(&super::tavily::TavilyProvider),
        "brave" => Some(&super::brave::BraveProvider),
        "exa" => Some(&super::exa::ExaProvider),
        "serper" => Some(&super::serper::SerperProvider),
        _ => None,
    }
}

/** 供应商清单，供设置归一化和前端选项保持同一来源。 */
pub(crate) fn provider_descriptor(id: &str) -> Option<&'static WebSearchProviderDescriptor> {
    WEB_SEARCH_PROVIDERS
        .iter()
        .find(|descriptor| descriptor.id == id)
}

/** 创建带统一超时和 UA 的搜索客户端。 */
pub(crate) fn search_client() -> Result<reqwest::Client, WebError> {
    reqwest::Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Orange/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| WebError::Network)
}

/** 把 HTTP 状态映射成统一的供应商错误；响应体不进入日志或模型。 */
pub(crate) fn map_response_status(status: reqwest::StatusCode) -> Result<(), WebError> {
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(WebError::InvalidCredentials);
    }
    if status.as_u16() == 429 {
        return Err(WebError::RateLimited);
    }
    if !status.is_success() {
        return Err(WebError::Backend);
    }
    Ok(())
}

/** 解析 JSON；解析失败只报后端错误，不暴露响应体。 */
pub(crate) async fn parse_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, WebError> {
    map_response_status(response.status())?;
    response.json().await.map_err(|_| WebError::Backend)
}

/** 统一过滤 URL、补齐标题并截断摘要。 */
pub(crate) fn normalize_hit(
    title: &str,
    url: &str,
    snippet: &str,
    published_at: Option<String>,
) -> Option<WebHit> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let title = {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            reqwest::Url::parse(url)
                .ok()
                .and_then(|parsed| parsed.host_str().map(str::to_owned))
                .unwrap_or_else(|| url.to_owned())
        } else {
            trimmed.to_owned()
        }
    };
    Some(WebHit {
        title,
        url: url.to_owned(),
        snippet: clip_chars(snippet, MAX_WEB_SNIPPET_CHARS),
        published_at: published_at
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::WebSearchSettings;
    use crate::web::normalize_web_search_settings;

    /** 保存设置时用后端清单钉死供应商和 keyring 引用，防止前端占位值造成密钥失联。 */
    #[test]
    fn normalize_pins_known_provider_and_key_reference() {
        let mut settings = WebSearchSettings {
            enabled: true,
            provider: "exa".to_owned(),
            key_reference: "frontend-placeholder".to_owned(),
            credential_status: "valid".to_owned(),
        };
        normalize_web_search_settings(&mut settings);
        assert_eq!(settings.provider, "exa");
        assert_eq!(settings.key_reference, "orange-web-search-exa");
    }

    /** 旧配置或未知 provider 静默迁移回 Tavily，避免 Agent 因设置损坏而失去联网能力。 */
    #[test]
    fn normalize_unknown_provider_falls_back_to_tavily() {
        let mut settings = WebSearchSettings {
            enabled: false,
            provider: "unknown".to_owned(),
            key_reference: "orange-web-search-brave".to_owned(),
            credential_status: "untested".to_owned(),
        };
        normalize_web_search_settings(&mut settings);
        assert_eq!(settings.provider, "tavily");
        assert_eq!(settings.key_reference, "orange-web-search-tavily");
    }

    /** 设置页和 keyring 引用都依赖清单稳定；重复 id 会让保存行为变得不可预测。 */
    #[test]
    fn provider_descriptors_have_unique_ids_and_key_references() {
        let mut ids = WEB_SEARCH_PROVIDERS
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<Vec<_>>();
        let mut key_references = WEB_SEARCH_PROVIDERS
            .iter()
            .map(|descriptor| descriptor.key_reference)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        key_references.sort_unstable();
        let id_count = ids.len();
        let key_reference_count = key_references.len();
        ids.dedup();
        key_references.dedup();
        assert_eq!(ids.len(), id_count);
        assert_eq!(key_references.len(), key_reference_count);
    }

    /** 清单里的每个供应商都必须真的可执行，避免设置页出现不可用选项。 */
    #[test]
    fn all_listed_providers_are_executable() {
        for descriptor in WEB_SEARCH_PROVIDERS {
            assert!(find_provider(descriptor.id).is_some(), "{}", descriptor.id);
        }
    }

    #[test]
    fn unknown_provider_is_rejected() {
        assert!(find_provider("unknown").is_none());
        assert!(provider_descriptor("unknown").is_none());
    }
}
