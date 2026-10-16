//! Brave Web Search 客户端。

use serde::Deserialize;

use super::provider::{
    map_response_status, normalize_hit, parse_json_response, search_client, WebSearchProvider,
};
use super::{WebError, WebHit};

const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub(crate) struct BraveProvider;

impl WebSearchProvider for BraveProvider {
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
        .get(ENDPOINT)
        .query(&[("q", query), ("count", limit.to_string())])
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                WebError::SearchTimeout
            } else {
                WebError::Network
            }
        })?;
    map_response_status(response.status())?;
    let payload: BraveResponse = parse_json_response(response).await?;
    Ok(payload
        .web
        .and_then(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .filter_map(normalize_brave_hit)
        .take(limit)
        .collect())
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWebSection>,
}

#[derive(Debug, Deserialize)]
struct BraveWebSection {
    #[serde(default)]
    results: Option<Vec<BraveResult>>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    age: Option<String>,
}

fn normalize_brave_hit(result: BraveResult) -> Option<WebHit> {
    normalize_hit(&result.title, &result.url, &result.description, result.age)
}
