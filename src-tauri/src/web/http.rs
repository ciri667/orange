//! 真实 HTTP。只在运行时线程里用 block_in_place 调用，测试走脚本替身。

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use futures_util::StreamExt;

use super::fetch::{HttpExchange, HttpResponse, MAX_RESPONSE_BYTES};
use super::WebError;

/** 抓页超时。搜索超时在 Tavily 客户端里单独限制。 */
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/** 生产环境的公网抓取。不发送 Cookie 或 Authorization。 */
pub(crate) struct LiveHttp;

impl HttpExchange for LiveHttp {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, WebError> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let addresses = (host, port)
            .to_socket_addrs()
            .map_err(|_| WebError::BlockedUrl)?
            .map(|address| address.ip())
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(WebError::BlockedUrl);
        }
        Ok(addresses)
    }

    fn exchange(&self, url: &str) -> Result<HttpResponse, WebError> {
        let url = url.to_owned();
        run_on_runtime(async move { exchange_once(&url).await })
    }
}

fn run_on_runtime<T>(
    work: impl std::future::Future<Output = Result<T, WebError>>,
) -> Result<T, WebError> {
    let handle = tokio::runtime::Handle::try_current().map_err(|_| WebError::Network)?;
    tokio::task::block_in_place(|| handle.block_on(work))
}

async fn exchange_once(url: &str) -> Result<HttpResponse, WebError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(format!("Orange/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| WebError::Network)?;
    let response = client.get(url).send().await.map_err(|error| {
        if error.is_timeout() {
            WebError::FetchTimeout
        } else {
            WebError::Network
        }
    })?;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if !(200..300).contains(&status) {
        return Ok(HttpResponse {
            status,
            location,
            content_type,
            body: Vec::new(),
            truncated_by_bytes: false,
        });
    }
    let (body, truncated_by_bytes) = read_capped(response).await?;
    Ok(HttpResponse {
        status,
        location,
        content_type,
        body,
        truncated_by_bytes,
    })
}

async fn read_capped(response: reqwest::Response) -> Result<(Vec<u8>, bool), WebError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| WebError::Network)?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            let room = MAX_RESPONSE_BYTES.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
}
