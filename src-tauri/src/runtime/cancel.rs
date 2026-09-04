//! 按会话登记 Agent 回合取消令牌。Stop 走独立 IPC，必须能打断正在跑的 `run_agent_turn`。

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::sync::watch;

/** 用户取消时返回的稳定错误文案，供 Provider 错误分类识别为 Abort。 */
pub const USER_ABORT_ERROR: &str = "request aborted by user";

/** 同一会话已有进行中回合时拒绝第二轮，避免 UI 与 IM 共用取消令牌。 */
pub const SESSION_TURN_ACTIVE_ERROR: &str = "当前会话仍有进行中的回合。";

static REGISTRY: OnceLock<Mutex<HashMap<String, AgentCancelEntry>>> = OnceLock::new();

/** 取消表条目：Stop 抢先到达时是占位，真正开跑后才是 Running。 */
#[derive(Clone, Debug)]
enum AgentCancelEntry {
    Placeholder(AgentCancel),
    Running(AgentCancel),
}

impl AgentCancelEntry {
    fn token(&self) -> &AgentCancel {
        match self {
            Self::Placeholder(token) | Self::Running(token) => token,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
}

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

fn lock_registry() -> MutexGuard<'static, HashMap<String, AgentCancelEntry>> {
    REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/** 为本会话开始一轮。已有预取消占位时复用；已有 Running 则拒绝。 */
pub fn try_register(session_id: &str) -> Result<AgentCancel, String> {
    let mut registry = lock_registry();
    match registry.get(session_id) {
        Some(AgentCancelEntry::Running(_)) => Err(SESSION_TURN_ACTIVE_ERROR.to_owned()),
        Some(AgentCancelEntry::Placeholder(existing)) => {
            let cancel = existing.clone();
            registry.insert(
                session_id.to_owned(),
                AgentCancelEntry::Running(cancel.clone()),
            );
            Ok(cancel)
        }
        None => {
            let cancel = AgentCancel::new();
            registry.insert(
                session_id.to_owned(),
                AgentCancelEntry::Running(cancel.clone()),
            );
            Ok(cancel)
        }
    }
}

/** 测试与旧调用约定：成功开始一轮，已有 Running 时直接失败。 */
pub fn register(session_id: &str) -> AgentCancel {
    try_register(session_id).expect(SESSION_TURN_ACTIVE_ERROR)
}

/** 该会话是否有尚未结束的 Running 回合；预取消占位不算。 */
pub fn is_session_turn_active(session_id: &str) -> bool {
    lock_registry()
        .get(session_id)
        .is_some_and(AgentCancelEntry::is_running)
}

/** 当前所有 Running 回合的会话 ID，供前端避免和 IM 后台回合撞车。 */
pub fn list_active_session_ids() -> Vec<String> {
    let mut session_ids = lock_registry()
        .iter()
        .filter(|(_, entry)| entry.is_running())
        .map(|(session_id, _)| session_id.clone())
        .collect::<Vec<_>>();
    session_ids.sort();
    session_ids
}

/** 取消该会话当前回合。尚无登记时写入已取消占位，避免 TOCTOU。 */
pub fn request_abort(session_id: &str) {
    let mut registry = lock_registry();
    if let Some(existing) = registry.get(session_id) {
        existing.token().abort();
        return;
    }
    let cancel = AgentCancel::new();
    cancel.abort();
    registry.insert(session_id.to_owned(), AgentCancelEntry::Placeholder(cancel));
}

/** 丢掉过期的预取消占位，避免上一轮结束后的 Stop 把下一轮开成已中断。 */
pub fn clear_placeholder(session_id: &str) {
    let mut registry = lock_registry();
    if matches!(
        registry.get(session_id),
        Some(AgentCancelEntry::Placeholder(_))
    ) {
        registry.remove(session_id);
    }
}

/** 回合结束时撤掉本回合令牌。预取消占位若已换成别的发送端则保留。 */
pub fn unregister(session_id: &str, token: &AgentCancel) {
    let mut registry = lock_registry();
    if registry
        .get(session_id)
        .is_some_and(|existing| existing.token().tx.same_channel(&token.tx))
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

    #[test]
    fn is_session_turn_active_tracks_register_and_unregister() {
        let session_id = isolated_session("active");
        assert!(!is_session_turn_active(&session_id));
        let cancel = register(&session_id);
        assert!(is_session_turn_active(&session_id));
        unregister(&session_id, &cancel);
        assert!(!is_session_turn_active(&session_id));
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

    #[test]
    fn try_register_rejects_second_turn_on_same_session_while_active() {
        let session_id = isolated_session("second-turn");
        let first = try_register(&session_id).expect("first turn");
        let second = try_register(&session_id);
        assert_eq!(second.unwrap_err(), SESSION_TURN_ACTIVE_ERROR);
        unregister(&session_id, &first);
        assert!(try_register(&session_id).is_ok());
    }

    #[test]
    fn try_register_promotes_pre_cancel_placeholder_instead_of_rejecting() {
        let session_id = isolated_session("placeholder");
        request_abort(&session_id);
        assert!(!is_session_turn_active(&session_id));
        let cancel = try_register(&session_id).expect("pre-cancel should start aborted turn");
        assert!(cancel.is_aborted());
        assert!(is_session_turn_active(&session_id));
        unregister(&session_id, &cancel);
    }

    #[test]
    fn allows_turns_on_two_sessions_without_shared_cancel_token() {
        let session_a = isolated_session("parallel-a");
        let session_b = isolated_session("parallel-b");
        let cancel_a = try_register(&session_a).expect("session a");
        let cancel_b = try_register(&session_b).expect("session b");
        request_abort(&session_a);
        assert!(cancel_a.is_aborted());
        assert!(!cancel_b.is_aborted());
        unregister(&session_a, &cancel_a);
        unregister(&session_b, &cancel_b);
    }

    #[test]
    fn abort_after_unregister_does_not_poison_the_next_turn() {
        let session_id = isolated_session("stale-placeholder");
        let first = try_register(&session_id).expect("first turn");
        unregister(&session_id, &first);
        request_abort(&session_id);
        clear_placeholder(&session_id);
        let second = try_register(&session_id).expect("next turn");
        assert!(!second.is_aborted());
        unregister(&session_id, &second);
    }

    #[test]
    fn list_active_session_ids_excludes_placeholders() {
        let running = isolated_session("list-running");
        let placeholder = isolated_session("list-placeholder");
        let cancel = try_register(&running).expect("running");
        request_abort(&placeholder);
        let active = list_active_session_ids();
        assert!(active.contains(&running));
        assert!(!active.contains(&placeholder));
        unregister(&running, &cancel);
    }
}
