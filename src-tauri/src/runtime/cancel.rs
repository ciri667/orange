//! 按会话登记 Agent 回合取消令牌。Stop 走独立 IPC，必须能打断正在跑的 `run_agent_turn`。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::sync::watch;

/** 用户取消时返回的稳定错误文案，供 Provider 错误分类识别为 Abort。 */
pub const USER_ABORT_ERROR: &str = "request aborted by user";

static REGISTRY: OnceLock<Mutex<HashMap<String, AgentCancel>>> = OnceLock::new();

/** 可克隆的回合取消句柄；`abort()` 后所有等待方立刻醒来。 */
#[derive(Clone, Debug)]
pub struct AgentCancel {
    tx: watch::Sender<bool>,
}

impl AgentCancel {
    fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self { tx }
    }

    /** 标记本回合已取消；重复调用是空操作。 */
    pub fn abort(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_aborted(&self) -> bool {
        *self.tx.borrow()
    }

    /** 直到取消或发送端被丢弃。已取消时立即返回。 */
    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            return;
        }
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    }
}

fn lock_registry() -> MutexGuard<'static, HashMap<String, AgentCancel>> {
    REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/** 为本会话登记取消令牌。已有预取消占位时复用，避免 Stop 抢在 register 前到达。 */
pub fn register(session_id: &str) -> AgentCancel {
    let mut registry = lock_registry();
    if let Some(existing) = registry.get(session_id) {
        return existing.clone();
    }
    let cancel = AgentCancel::new();
    registry.insert(session_id.to_owned(), cancel.clone());
    cancel
}

/** 取消该会话当前回合。尚无登记时写入已取消占位，避免 TOCTOU。 */
pub fn request_abort(session_id: &str) {
    let mut registry = lock_registry();
    if let Some(existing) = registry.get(session_id) {
        existing.abort();
        return;
    }
    let cancel = AgentCancel::new();
    cancel.abort();
    registry.insert(session_id.to_owned(), cancel);
}

/** 回合结束时撤掉本回合令牌。预取消占位若已换成别的发送端则保留。 */
pub fn unregister(session_id: &str, token: &AgentCancel) {
    let mut registry = lock_registry();
    if registry
        .get(session_id)
        .is_some_and(|existing| existing.tx.same_channel(&token.tx))
    {
        registry.remove(session_id);
    }
}

/** 离开 `run_agent_turn` 时注销令牌，包括提前 return 和 panic。 */
pub struct AgentCancelGuard {
    session_id: String,
    token: AgentCancel,
}

impl AgentCancelGuard {
    pub fn new(session_id: impl Into<String>, token: AgentCancel) -> Self {
        Self {
            session_id: session_id.into(),
            token,
        }
    }
}

impl Drop for AgentCancelGuard {
    fn drop(&mut self) {
        unregister(&self.session_id, &self.token);
    }
}

pub fn is_user_abort_error(error: &str) -> bool {
    error == USER_ABORT_ERROR
        || error
            .to_ascii_lowercase()
            .contains("request aborted by user")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_session(label: &str) -> String {
        format!("session-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn abort_without_register_is_observed_by_later_register() {
        let session_id = isolated_session("pre-cancel");
        request_abort(&session_id);
        let cancel = register(&session_id);
        assert!(cancel.is_aborted());
    }

    #[test]
    fn abort_after_register_flips_token() {
        let session_id = isolated_session("live");
        let cancel = register(&session_id);
        assert!(!cancel.is_aborted());
        request_abort(&session_id);
        assert!(cancel.is_aborted());
    }

    #[test]
    fn unregister_does_not_abort_a_later_turn() {
        let session_id = isolated_session("next-turn");
        let first = register(&session_id);
        request_abort(&session_id);
        unregister(&session_id, &first);
        let second = register(&session_id);
        assert!(!second.is_aborted());
    }

    #[test]
    fn abort_unknown_session_is_not_an_error() {
        request_abort(&isolated_session("missing"));
    }

    #[tokio::test]
    async fn cancelled_wakes_waiters() {
        let session_id = isolated_session("wake");
        let cancel = register(&session_id);
        let waiter = cancel.clone();
        let wait = tokio::spawn(async move {
            waiter.cancelled().await;
        });
        request_abort(&session_id);
        wait.await.expect("waiter should observe abort");
        assert!(cancel.is_aborted());
    }
}
