//! Tavily 搜索客户端。密钥只放在 Authorization 头，不进入查询字符串。

use serde::Deserialize;

use super::provider::{normalize_hit, parse_json_response, search_client, WebSearchProvider};
use super::{WebError, WebHit};

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

/** Tavily 是默认搜索供应商。 */
pub(crate) struct TavilyProvider;

impl WebSearchProvider for TavilyProvider {
    /** 向 Tavily 查询。limit 由调用方决定，这里按传入条数请求。 */
    fn search(&self, api_key: &str, query: &str, limit: usize) -> Result<Vec<WebHit>, WebError> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| WebError::Network)?;
        let api_key = api_key.to_owned();
        let query = query.to_owned();
        tokio::task::block_in_place(|| handle.block_on(search_async(api_key, query, limit)))
    }
}

async fn search_async(
    api_key: String,
    query: String,
    limit: usize,
) -> Result<Vec<WebHit>, WebError> {
    let response = search_client()?
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
    let payload: TavilyResponse = parse_json_response(response).await?;
    Ok(payload
        .results
        .into_iter()
        .filter_map(normalize_tavily_hit)
        .take(limit)
        .collect())
}

fn normalize_tavily_hit(result: TavilyResult) -> Option<WebHit> {
    normalize_hit(
        &result.title,
        &result.url,
        &result.content,
        result.published_date,
    )
}
