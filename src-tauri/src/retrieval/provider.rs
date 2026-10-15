//! Embedding HTTP 适配。错误只返回固定分类，服务响应体和输入不得进入日志。
use crate::domain::SemanticSearchSettings;
use futures_util::StreamExt;
use reqwest::Url;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Debug)]
pub(crate) struct EmbeddingError {
    pub message: &'static str,
    pub retryable: bool,
    pub batch_unsupported: bool,
    pub configuration: bool,
}

impl EmbeddingError {
    fn invalid() -> Self {
        Self {
            message: "向量服务返回了无效向量。",
            retryable: false,
            batch_unsupported: false,
            configuration: true,
        }
    }
}

/** local 服务强制回环 IP（localhost 固定解析为 127.0.0.1），防止 DNS 或代理绕过本地边界。 */
pub(crate) fn endpoint(settings: &SemanticSearchSettings) -> Result<(Url, bool), String> {
    if !matches!(
        settings.provider.as_str(),
        "ollama" | "lm-studio" | "openai-compatible"
    ) {
        return Err("不支持的向量服务类型。".into());
    }
    let mut url = Url::parse(settings.api_base.trim()).map_err(|_| "向量服务地址无效。")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("向量服务地址只接受不含凭证、查询参数的 HTTP(S) 地址。".into());
    }
    if url.host_str() == Some("localhost") {
        url.set_host(Some("127.0.0.1"))
            .map_err(|_| "本地地址无效。")?;
    }
    let local = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if settings.provider != "openai-compatible" && !local {
        return Err("本地向量服务只允许本机回环地址；远程服务请选择云端兼容接口。".into());
    }
    if !local && url.scheme() != "https" {
        return Err("云端向量服务必须使用 HTTPS。".into());
    }
    let base = url.path().trim_end_matches('/');
    let path = if settings.provider == "ollama" {
        if base.ends_with("/api/embed") {
            base.to_owned()
        } else if base.ends_with("/api") {
            format!("{base}/embed")
        } else {
            format!("{base}/api/embed")
        }
    } else if base.ends_with("/embeddings") {
        base.to_owned()
    } else {
        format!("{base}/embeddings")
    };
    url.set_path(&path);
    Ok((url, !local))
}

/** 使用与请求相同的归一化端点；输入格式版本变化会自然触发重建。 */
pub(crate) fn fingerprint(settings: &SemanticSearchSettings) -> String {
    let address = endpoint(settings)
        .map(|(url, _)| url.to_string())
        .unwrap_or_else(|_| settings.api_base.trim().to_owned());
    crate::storage::hash_content(&format!(
        "embedding-input-v1\n{}\n{address}\n{}",
        settings.provider,
        settings.model.trim()
    ))
}

pub(crate) fn validate(settings: &SemanticSearchSettings) -> Result<(), String> {
    endpoint(settings)?;
    if settings.model.trim().is_empty() {
        return Err("请填写 embedding 模型名称。".into());
    }
    if !settings.min_similarity.is_finite() || !(-1.0..=1.0).contains(&settings.min_similarity) {
        return Err("相似度下限必须在 -1 到 1 之间。".into());
    }
    Ok(())
}

/** 校验数量、顺序、有限值和维度，并统一归一化，便于本地流式点积。 */
pub(crate) fn parse_vectors(
    body: &Value,
    ollama: bool,
    count: usize,
) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    let values: Vec<&Value> = if ollama {
        body.get("embeddings")
            .and_then(Value::as_array)
            .ok_or_else(EmbeddingError::invalid)?
            .iter()
            .collect()
    } else {
        let rows = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(EmbeddingError::invalid)?;
        let mut ordered = vec![None; count];
        for row in rows {
            let index = row
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(EmbeddingError::invalid)? as usize;
            if index >= count || ordered[index].is_some() {
                return Err(EmbeddingError::invalid());
            }
            ordered[index] = row.get("embedding");
        }
        ordered
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(EmbeddingError::invalid)?
    };
    if values.len() != count {
        return Err(EmbeddingError::invalid());
    }
    let mut dimensions = None;
    values
        .into_iter()
        .map(|value| {
            let array = value.as_array().ok_or_else(EmbeddingError::invalid)?;
            if array.is_empty()
                || array.len() > 32768
                || dimensions.is_some_and(|d| d != array.len())
            {
                return Err(EmbeddingError::invalid());
            }
            dimensions = Some(array.len());
            let mut vector: Vec<f32> = array
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite())
                        .ok_or_else(EmbeddingError::invalid)
                })
                .collect::<Result<_, _>>()?;
            let norm = vector
                .iter()
                .map(|n| (*n as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            if norm <= f64::EPSILON || !norm.is_finite() {
                return Err(EmbeddingError::invalid());
            }
            vector
                .iter_mut()
                .for_each(|n| *n = (*n as f64 / norm) as f32);
            Ok(vector)
        })
        .collect()
}

/** 异步请求仅占网络资源；调用方负责 scope、云端授权、任务版本与事务边界。 */
pub(crate) async fn embed(
    settings: &SemanticSearchSettings,
    key: &str,
    inputs: &[String],
    timeout: Duration,
) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    let (url, remote) = endpoint(settings).map_err(|_| EmbeddingError {
        message: "向量服务地址无效。",
        ..EmbeddingError::invalid()
    })?;
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none());
    if !remote {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| EmbeddingError::invalid())?;
    let input = if inputs.len() == 1 {
        json!(inputs[0])
    } else {
        json!(inputs)
    };
    let body = if settings.provider == "ollama" {
        json!({"model": settings.model, "input": input, "truncate": false})
    } else {
        json!({"model": settings.model, "input": input, "encoding_format": "float"})
    };
    let mut request = client.post(url).json(&body);
    if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|error| EmbeddingError {
        message: if error.is_timeout() {
            "向量服务请求超时。"
        } else {
            "无法连接向量服务。"
        },
        retryable: true,
        batch_unsupported: false,
        configuration: false,
    })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(EmbeddingError {
            message: match status {
                401 | 403 => "向量服务密钥无效或权限不足。",
                404 => "向量模型或接口不存在。",
                429 => "向量服务限流，请稍后重试。",
                400 | 413 | 422 => "向量服务拒绝了输入，请检查模型和输入长度。",
                _ => "向量服务暂时不可用。",
            },
            retryable: status == 429 || status >= 500,
            batch_unsupported: inputs.len() > 1 && matches!(status, 400 | 413 | 422),
            configuration: matches!(status, 401 | 403 | 404),
        });
    }
    // 不读取无界响应；最多允许一批 16 个高维 float 向量。
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| EmbeddingError {
            message: "读取向量服务响应失败。",
            retryable: true,
            batch_unsupported: false,
            configuration: false,
        })?;
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err(EmbeddingError::invalid());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| EmbeddingError::invalid())?;
    parse_vectors(&body, settings.provider == "ollama", inputs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatible_response_restores_input_order_and_normalizes() {
        let vectors = parse_vectors(
            &json!({"data":[{"index":1,"embedding":[0,2]},{"index":0,"embedding":[3,0]}]}),
            false,
            2,
        )
        .unwrap();
        assert_eq!(vectors, vec![vec![1., 0.], vec![0., 1.]]);
    }
    #[test]
    fn rejects_missing_duplicate_zero_and_mismatched_vectors() {
        for body in [
            json!({"embeddings": [[0,0]]}),
            json!({"embeddings": [[1,0],[1]]}),
            json!({"embeddings": []}),
        ] {
            assert!(parse_vectors(&body, true, 1).is_err());
        }
        assert!(parse_vectors(
            &json!({"data":[{"index":0,"embedding":[1]},{"index":0,"embedding":[1]}]}),
            false,
            2
        )
        .is_err());
        assert!(parse_vectors(&json!({"embeddings":[[1e300]]}), true, 1).is_err());
    }
    #[test]
    fn local_endpoint_never_accepts_remote_or_credentials() {
        let mut settings = SemanticSearchSettings::default();
        for address in [
            "https://example.com",
            "http://192.168.1.2:11434",
            "http://user@127.0.0.1",
            "file:///tmp/x",
        ] {
            settings.api_base = address.into();
            assert!(endpoint(&settings).is_err());
        }
        settings.api_base = "http://localhost:11434/".into();
        assert_eq!(
            endpoint(&settings).unwrap().0.as_str(),
            "http://127.0.0.1:11434/api/embed"
        );
    }
}
