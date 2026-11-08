//! Agent 模型请求的可重试失败与退避策略。
//!
//! 只负责「要不要等、等多久」，不决定哪些错误可重试；错误分类仍由
//! `provider_error` 统一负责。这样退避参数可以单独测试，也避免 runtime 主循环
//! 里散落魔法数字。

use crate::runtime::stream::StreamedAssistant;
use std::time::{Duration, SystemTime};

/// 首次失败后最多再发起多少次请求。
///
/// 6 次配合指数退避约等待 1 + 2 + 4 + 8 + 16 + 30 ≈ 61 秒；provider 短暂过载时
/// 用户通常还没失去耐心，又不会像原来那样 3 秒内把 4 次机会全部打光。
pub(super) const MAX_RETRY_ATTEMPTS: u32 = 6;

/// 第一次重试的等待时间，之后按 2 倍递增。
const BASE_DELAY: Duration = Duration::from_secs(1);

/// 单次等待上限。服务端 `Retry-After` 再长也不让界面干等超过这个值。
const MAX_DELAY: Duration = Duration::from_secs(30);

/// 首次失败后允许累计等待的窗口；窗口用完就停止重试并给出可读错误。
///
/// 只累计退避等待时间，不把单次请求本身的耗时算进去：长文流式请求可能本身
/// 就要跑几分钟，用总耗时做预算会误伤正常重试。
pub(super) const TOTAL_WAIT_BUDGET: Duration = Duration::from_secs(120);

/// 从第一次尝试开始允许的总耗时上限（含请求本身）；只在失败后检查。
///
/// 读超时是 180 秒，如果连续几次都卡到超时，仅靠等待预算会叠成十几分钟。
/// 这里不主动打断正在进行的长生成，只是不给已经跑了很久的失败请求继续重试。
pub(super) const TOTAL_REQUEST_WINDOW: Duration = Duration::from_secs(300);

/// 抖动比例上限：只在计算值上增加 0–20%，保证不会比预期更早重试。
const MAX_JITTER_RATIO: f64 = 0.20;

/// 一次可重试失败的完整信息；`retry_after` 来自 HTTP `Retry-After` 响应头。
#[derive(Clone, Debug)]
pub(super) struct ModelRequestFailure {
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl ModelRequestFailure {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
        }
    }

    pub(super) fn with_retry_after(
        message: impl Into<String>,
        retry_after: Option<Duration>,
    ) -> Self {
        Self {
            message: message.into(),
            retry_after,
        }
    }
}

impl From<String> for ModelRequestFailure {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ModelRequestFailure {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// 重试等待事件，运行时把它翻译成用户可见的「约 N 秒后重试」提示。
#[derive(Clone, Copy, Debug)]
pub(super) struct RetryNotice {
    pub retry_index: u32,
    pub max_retries: u32,
    pub delay: Duration,
}

/// 请求过程中的事件；由调用方决定如何映射到主 Agent 或子 Agent 的过程区。
pub(super) enum ModelRequestEvent<'a> {
    /// 收到一段流式增量；与原先的 `on_progress` 回调等价。
    Streamed(&'a StreamedAssistant),
    /// 即将进入退避等待。
    RetryWaiting(RetryNotice),
    /// 本次请求已结束（成功、失败或用户取消），如果之前展示过等待提示就清掉。
    RetryFinished,
}

/// 计算第 `retry_index` 次重试（从 1 开始）的等待时长。
///
/// - provider 给了 `Retry-After` 时以服务端为准，但受单次上限约束；
/// - 否则按 1s、2s、4s…… 指数退避，最高 30s；
/// - `jitter` 是 0–1 的随机数，只用于放大等待，避免多个会话同一瞬间重试。
pub(super) fn retry_delay(
    retry_index: u32,
    server_hint: Option<Duration>,
    jitter: f64,
) -> Duration {
    let exponent = retry_index.saturating_sub(1).min(5);
    let backoff = BASE_DELAY.saturating_mul(1u32 << exponent).min(MAX_DELAY);
    let base = server_hint
        .map(|hint| hint.min(MAX_DELAY))
        .unwrap_or(backoff)
        // 少数网关会返回 `Retry-After: 0`；至少等 1 秒，避免瞬间打满重试次数。
        .max(BASE_DELAY);
    let jitter = jitter.clamp(0.0, 1.0);
    base.saturating_add(base.mul_f64(MAX_JITTER_RATIO * jitter))
}

/// 用系统时间生成 0–1 的抖动系数，避免为了一个随机数引入额外依赖。
pub(super) fn next_jitter() -> f64 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    // xorshift：时间低位已经够随机，这里只做一次扩散，避免相邻重试拿到相同系数。
    let mut value = nanos | 1;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    f64::from(value % 10_000) / 10_000.0
}

/// 解析 HTTP `Retry-After`：既支持秒数，也支持 RFC 2822/HTTP-date。
///
/// 返回值只表示「建议等待多久」；是否真的等待由调用方结合总预算决定。
pub(super) fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let parsed = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let target = parsed.with_timezone(&chrono::Utc);
    let now = chrono::DateTime::<chrono::Utc>::from(now);
    if target <= now {
        return Some(Duration::ZERO);
    }
    (target - now).to_std().ok()
}

/// 重试次数/预算用尽时，在原始错误后面补上重试信息。
///
/// 这里不拼接「模型服务暂时不可用」等最终文案，交给 `provider_error` 统一转换，
/// 避免调用链上重复加前缀。
pub(super) fn annotate_retry_exhausted(error: &str, retries: u32, waited: Duration) -> String {
    if retries == 0 {
        return error.to_owned();
    }
    format!(
        "{error}（已自动重试 {retries} 次，累计等待约 {} 秒）",
        waited.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_caps_at_max_delay() {
        assert_eq!(retry_delay(1, None, 0.0), Duration::from_secs(1));
        assert_eq!(retry_delay(2, None, 0.0), Duration::from_secs(2));
        assert_eq!(retry_delay(3, None, 0.0), Duration::from_secs(4));
        assert_eq!(retry_delay(7, None, 0.0), Duration::from_secs(30));
        assert_eq!(retry_delay(20, None, 0.0), Duration::from_secs(30));
    }

    #[test]
    fn default_schedule_waits_about_a_minute_and_fits_the_wait_budget() {
        let total: Duration = (1..=MAX_RETRY_ATTEMPTS)
            .map(|index| retry_delay(index, None, 0.0))
            .sum();
        assert_eq!(total, Duration::from_secs(61));
        assert!(total <= TOTAL_WAIT_BUDGET);
    }

    #[test]
    fn server_hint_overrides_backoff_and_still_respects_cap() {
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(12)), 0.0),
            Duration::from_secs(12)
        );
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(600)), 0.0),
            Duration::from_secs(30)
        );
        assert_eq!(
            retry_delay(1, Some(Duration::ZERO), 0.0),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn jitter_only_lengthens_the_wait() {
        let base = retry_delay(3, None, 0.0);
        let jittered = retry_delay(3, None, 1.0);
        assert!(jittered > base);
        assert_eq!(jittered, Duration::from_millis(4_800));
    }

    #[test]
    fn parses_retry_after_seconds_and_http_date() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_445_412_300);
        assert_eq!(parse_retry_after("7", now), Some(Duration::from_secs(7)));
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(Duration::from_secs(180))
        );
        assert_eq!(
            parse_retry_after(
                "Wed, 21 Oct 2015 07:28:00 GMT",
                now + Duration::from_secs(3600)
            ),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("nonsense", now), None);
    }

    #[test]
    fn exhausted_message_keeps_original_error_and_retry_count() {
        let text = annotate_retry_exhausted("HTTP 503 busy", 6, Duration::from_secs(61));
        assert!(text.contains("HTTP 503 busy"));
        assert!(text.contains("已自动重试 6 次"));
        assert!(text.contains("61 秒"));
        assert_eq!(
            annotate_retry_exhausted("HTTP 429 rate limit", 0, Duration::ZERO),
            "HTTP 429 rate limit"
        );
    }
}
