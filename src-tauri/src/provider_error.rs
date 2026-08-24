use serde_json::Value;

/** Provider 失败分类，决定循环是重试、压缩还是直接结束。 */
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderErrorKind {
    /** 用户或运行时取消，不重试。 */
    Abort,
    /** 配额或账单问题，不重试。 */
    Quota,
    /** 上下文超窗，去掉失败 assistant 后 compact，最多再请求一次。 */
    Overflow,
    /** 429 / 5xx / 断流，指数退避后可重试。 */
    Retryable,
    /** 其它错误，直接结束为本轮失败。 */
    Other,
}

/** 从 chat completions 响应解析 finish_reason，兼容 stop_reason。 */
pub fn parse_finish_reason(response: &Value) -> Option<String> {
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice
                .get("finish_reason")
                .or_else(|| choice.get("stop_reason"))
                .or_else(|| choice.get("native_finish_reason"))
                .and_then(Value::as_str)
        })
        .map(|reason| reason.trim().to_ascii_lowercase())
        .filter(|reason| !reason.is_empty())
}

/** length / max_tokens 表示模型输出被截断，本批 tool call 不得执行。 */
pub fn is_length_stop(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("length") | Some("max_tokens") | Some("max_output_tokens")
    )
}

/** 根据脱敏后的错误正文和可选 HTTP 状态分类 Provider 失败。 */
pub fn classify_provider_error(text: &str, http_status: Option<u16>) -> ProviderErrorKind {
    if let Some(status) = http_status {
        if status == 402 {
            return ProviderErrorKind::Quota;
        }
        if status == 429 || (500..600).contains(&status) {
            return ProviderErrorKind::Retryable;
        }
    }

    let lower = text.to_ascii_lowercase();
    if lower.contains("abort") || lower.contains("canceled") || lower.contains("cancelled") {
        return ProviderErrorKind::Abort;
    }
    if lower.contains("insufficient_quota")
        || lower.contains("billing")
        || lower.contains("payment required")
        || lower.contains("exceeded your current quota")
        || lower.contains("http 402")
    {
        return ProviderErrorKind::Quota;
    }
    if lower.contains("context_length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
        || lower.contains("token limit")
        || lower.contains("maximum context length")
    {
        return ProviderErrorKind::Overflow;
    }
    if lower.contains("http 429")
        || lower.contains("rate limit")
        || lower.contains("http 5")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("connection")
        || lower.contains("broken pipe")
        || lower.contains("error sending request")
        || lower.contains("无法发送模型请求")
    {
        return ProviderErrorKind::Retryable;
    }

    ProviderErrorKind::Other
}

/** 从「HTTP 429 ...」这类错误正文里抽出状态码。 */
pub fn parse_http_status_from_error(text: &str) -> Option<u16> {
    let marker = "HTTP ";
    let start = text.find(marker)? + marker.len();
    let status: String = text[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    status.parse().ok()
}

/** 给用户看的失败说明；配额和超窗要可读，不要只说「模型请求失败」。 */
pub fn user_facing_provider_error(text: &str) -> String {
    match classify_provider_error(text, parse_http_status_from_error(text)) {
        ProviderErrorKind::Abort => "模型请求已取消。".to_owned(),
        ProviderErrorKind::Quota => "模型额度或账单不可用，请检查 Provider 配额后再试。".to_owned(),
        ProviderErrorKind::Overflow => {
            "模型上下文超出窗口。已尝试压缩后仍失败，请缩短输入或新开会话。".to_owned()
        }
        ProviderErrorKind::Retryable => {
            format!(
                "模型服务暂时不可用：{}",
                crate::model_provider::redact_model_error_text(text)
            )
        }
        ProviderErrorKind::Other => crate::model_provider::redact_model_error_text(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_finish_reason_reads_openai_and_compat_fields() {
        let openai = json!({ "choices": [{ "finish_reason": "length", "message": {} }] });
        assert_eq!(parse_finish_reason(&openai).as_deref(), Some("length"));
        let compat = json!({ "choices": [{ "stop_reason": "max_tokens", "message": {} }] });
        assert_eq!(parse_finish_reason(&compat).as_deref(), Some("max_tokens"));
        assert!(is_length_stop(parse_finish_reason(&openai).as_deref()));
        assert!(is_length_stop(parse_finish_reason(&compat).as_deref()));
        assert!(!is_length_stop(Some("stop")));
    }

    #[test]
    fn classify_quota_overflow_retryable_and_abort() {
        assert_eq!(
            classify_provider_error("insufficient_quota for this API key", None),
            ProviderErrorKind::Quota
        );
        assert_eq!(
            classify_provider_error("HTTP 402 payment required", Some(402)),
            ProviderErrorKind::Quota
        );
        assert_eq!(
            classify_provider_error("This model's maximum context length is 8192 tokens", None),
            ProviderErrorKind::Overflow
        );
        assert_eq!(
            classify_provider_error("模型请求失败：HTTP 429 too many requests", Some(429)),
            ProviderErrorKind::Retryable
        );
        assert_eq!(
            classify_provider_error("模型请求失败：HTTP 503 unavailable", Some(503)),
            ProviderErrorKind::Retryable
        );
        assert_eq!(
            classify_provider_error("request aborted by client", None),
            ProviderErrorKind::Abort
        );
        assert_eq!(
            classify_provider_error("模型请求失败：HTTP 400 bad request", Some(400)),
            ProviderErrorKind::Other
        );
    }

    #[test]
    fn parse_http_status_from_error_reads_embedded_code() {
        assert_eq!(
            parse_http_status_from_error("模型请求失败：HTTP 429 rate limit"),
            Some(429)
        );
        assert_eq!(parse_http_status_from_error("network down"), None);
    }
}
