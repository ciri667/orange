//! Serper Google Search 客户端。

use serde::Deserialize;

use super::provider::{normalize_hit, parse_json_response, search_client, WebSearchProvider};
use super::{WebError, WebHit};

const ENDPOINT: &str = "https://google.serper.dev/search";

pub(crate) struct SerperProvider;

impl WebSearchProvider for SerperProvider {
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
        .header("X-API-KEY", api_key)
        .json(&serde_json::json!({ "q": query, "num": limit }))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                WebError::SearchTimeout
            } else {
                WebError::Network
            }
        })?;
    let payload: SerperResponse = parse_json_response(response).await?;
    Ok(payload
        .organic
        .into_iter()
        .filter_map(normalize_serper_hit)
        .take(limit)
        .collect())
}

#[derive(Debug, Deserialize)]
struct SerperResponse {
    #[serde(default)]
    organic: Vec<SerperResult>,
}

#[derive(Debug, Deserialize)]
struct SerperResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    snippet: String,
    #[serde(default)]
    date: Option<String>,
}

fn normalize_serper_hit(result: SerperResult) -> Option<WebHit> {
    normalize_hit(&result.title, &result.link, &result.snippet, result.date)
}
