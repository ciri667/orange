//! Exa 搜索客户端。

use serde::Deserialize;

use super::provider::{normalize_hit, parse_json_response, search_client, WebSearchProvider};
use super::{WebError, WebHit};

const ENDPOINT: &str = "https://api.exa.ai/search";

pub(crate) struct ExaProvider;

impl WebSearchProvider for ExaProvider {
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
        .post(ENDPOINT)
        .header("x-api-key", api_key)
        .json(&serde_json::json!({
            "query": query,
            "numResults": limit,
            "contents": { "text": true },
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
    let payload: ExaResponse = parse_json_response(response).await?;
    Ok(payload
        .results
        .into_iter()
        .filter_map(normalize_exa_hit)
        .take(limit)
        .collect())
}

#[derive(Debug, Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    published_date: Option<String>,
}

fn normalize_exa_hit(result: ExaResult) -> Option<WebHit> {
    normalize_hit(
        &result.title.unwrap_or_default(),
        &result.url,
        &result.text.unwrap_or_default(),
        result.published_date,
    )
}
