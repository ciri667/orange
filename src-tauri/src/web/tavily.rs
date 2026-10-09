//! Tavily 搜索客户端。密钥只放在 Authorization 头，不进入查询字符串。

use std::time::Duration;

use serde::Deserialize;

use super::{clip_chars, WebError, WebHit, MAX_WEB_SNIPPET_CHARS};

const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);
const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";

#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    published_date: Option<String>,
}

/** 向 Tavily 查询。limit 由调用方决定，这里按传入条数请求。 */
pub(crate) fn search(api_key: &str, query: &str, limit: usize) -> Result<Vec<WebHit>, WebError> {
    let handle = tokio::runtime::Handle::try_current().map_err(|_| WebError::Network)?;
    let api_key = api_key.to_owned();
    let query = query.to_owned();
    tokio::task::block_in_place(|| handle.block_on(search_async(api_key, query, limit)))
}

async fn search_async(
    api_key: String,
    query: String,
    limit: usize,
) -> Result<Vec<WebHit>, WebError> {
    let client = reqwest::Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Orange/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| WebError::Network)?;
    let response = client
        .post(TAVILY_ENDPOINT)
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "query": query,
            "max_results": limit,
            "search_depth": "basic",
            "include_answer": false,
            "include_raw_content": false,
        }))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                WebError::SearchTimeout
            } else {
                WebError::Network
            }
        })?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(WebError::InvalidCredentials);
    }
    if status.as_u16() == 429 {
        return Err(WebError::RateLimited);
    }
    if !status.is_success() {
        return Err(WebError::Backend);
    }
    let payload: TavilyResponse = response.json().await.map_err(|_| WebError::Backend)?;
    Ok(payload
        .results
        .into_iter()
        .filter_map(normalize_hit)
        .take(limit)
        .collect())
}

fn normalize_hit(result: TavilyResult) -> Option<WebHit> {
    let url = result.url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let title = {
        let trimmed = result.title.trim();
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
        snippet: clip_chars(&result.content, MAX_WEB_SNIPPET_CHARS),
        published_at: result
            .published_date
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
    })
}
