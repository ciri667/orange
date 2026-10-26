//! 一次性操作授权：前端只提交请求 ID 和决定，实际参数始终保留在后端。
use crate::{domain::AgentSession, permissions::{self, Decision}, runtime::AgentCancel};
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, sync::{Mutex, OnceLock}, time::Duration};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

static PENDING: OnceLock<Mutex<HashMap<String, (String, oneshot::Sender<bool>)>>> = OnceLock::new();
fn board() -> &'static Mutex<HashMap<String, (String, oneshot::Sender<bool>)>> {
    PENDING.get_or_init(Default::default)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Request { id: String, session_id: String, permission_version: u64, tool_name: String, arguments: Value, warning: &'static str }

/** Permit 不可由 IPC 反序列化，且只对精确会话、权限版本与参数有效。 */
pub struct Permit { session: AgentSession, name: String, args: Value }
impl Permit {
    pub fn matches(&self, session: &AgentSession, name: &str, args: &Value) -> bool {
        permissions::same_authority(&self.session, session) && self.name == name && self.args == *args
    }
    /** 异步执行入口必须持有 Runtime 发出的许可，并在真正访问前核对持久化权限。 */
    pub fn validate(&self, app: &AppHandle, session: &AgentSession, name: &str, args: &Value, cancel: &AgentCancel) -> Result<(),String> {
        check_current(app,session)?;
        if cancel.is_aborted() { return Err("回合已停止。".into()); }
        if !self.matches(session,name,args) { return Err("操作许可与当前参数或权限不匹配。".into()); }
        crate::permissions::ensure(session,crate::permissions::tool_operation(name,args))
    }
}

pub fn decide(id: &str, approved: bool) -> bool {
    board().lock().unwrap_or_else(|e| e.into_inner()).remove(id)
        .is_some_and(|(_, sender)| sender.send(approved).is_ok())
}
pub fn cancel_session(session_id: &str) {
    board().lock().unwrap_or_else(|e| e.into_inner()).retain(|_, (id, _)| id != session_id);
}

/** 一次性等待与清理共用实现，迟到决定拿不到发送端。 */
async fn wait_for_decision(id: &str, rx: oneshot::Receiver<bool>, cancel: &AgentCancel, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let approved = tokio::select! {
        decision = rx => decision.unwrap_or(false),
        _ = tokio::time::sleep_until(deadline) => false,
        _ = cancel.cancelled() => false,
    };
    decide(id, false);
    // 发送端与超时可能同时就绪；不能依赖 select 的随机分支顺序放行迟到决定。
    approved && tokio::time::Instant::now() < deadline && !cancel.is_aborted()
}

/** 操作前及批准后均从 SQLite 验证权限；拒绝、超时、取消和迟到决定不会执行。 */
pub async fn authorize(app: &AppHandle, session: &AgentSession, name: &str, args: &Value,
    decision: Decision, cancel: &AgentCancel) -> Result<Permit, String> {
    check_current(app, session)?;
    if cancel.is_aborted() { return Err("回合已停止。".into()); }
    if let Decision::Deny(reason) = decision { return Err(reason.into()); }
    if decision == Decision::RequireApproval {
        let id = crate::storage::create_id("operation");
        let (tx, rx) = oneshot::channel();
        board().lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), (session.id.clone(), tx));
        let request = Request { id: id.clone(), session_id: session.id.clone(), permission_version: session.permission_version,
            tool_name: name.to_owned(), arguments: args.clone(), warning: if name == "exec" {
                "批准后命令将在当前系统用户的真实环境执行。命令内部修改不逐项展示 diff；失败、取消也可能已产生副作用。不会自动提权。"
            } else { "仅批准本次准确参数；不建立目录、站点或服务免批授权。" } };
        if let Err(error) = app.emit("operation-approval", request) { decide(&id, false); return Err(error.to_string()); }
        let approved = wait_for_decision(&id, rx, cancel, Duration::from_secs(300)).await;
        let _ = app.emit("operation-approval-closed", &id);
        if !approved || cancel.is_aborted() { return Err("操作已拒绝、取消或等待审批超时。".into()); }
        check_current(app, session)?;
    }
    Ok(Permit { session: session.clone(), name: name.to_owned(), args: args.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pending(session_id: &str) -> (String, oneshot::Receiver<bool>) {
        let id = crate::storage::create_id("approval-test");
        let (tx,rx) = oneshot::channel();
        board().lock().unwrap().insert(id.clone(),(session_id.into(),tx));
        (id,rx)
    }

    #[tokio::test]
    async fn ready_approval_cannot_win_over_an_expired_deadline() {
        for _ in 0..20 {
            let (id,rx) = pending("deadline-race-test");
            assert!(decide(&id,true));
            assert!(!wait_for_decision(&id,rx,&AgentCancel::testing_token(),Duration::ZERO).await);
        }
    }

    #[tokio::test]
    async fn rejection_timeout_cancel_and_late_decisions_cannot_approve() {
        let cancel = AgentCancel::testing_token();
        let (id,rx) = pending("denial-test");
        assert!(decide(&id,false));
        assert!(!decide(&id,true));
        assert!(!wait_for_decision(&id,rx,&cancel,Duration::from_secs(1)).await);
        let (id,rx) = pending("timeout-test");
        assert!(!wait_for_decision(&id,rx,&cancel,Duration::from_millis(1)).await);
        assert!(!decide(&id,true));
        let (id,rx) = pending("revoke-test");
        cancel_session("revoke-test");
        assert!(!wait_for_decision(&id,rx,&cancel,Duration::from_secs(1)).await);
        assert!(!decide(&id,true));
        let (id,rx) = pending("stop-test");
        cancel.abort();
        assert!(!wait_for_decision(&id,rx,&cancel,Duration::from_secs(1)).await);
        assert!(!decide(&id,true));
        assert!(!decide("unknown-or-previous-process-id",true));
    }

    #[test]
    fn permit_binds_exact_session_authority_tool_and_arguments() {
        let session = crate::permissions::test_session("full-confirm");
        let args = json!({"path":"/a"});
        let permit = Permit {session:session.clone(),name:"read".into(),args:args.clone()};
        assert!(permit.matches(&session,"read",&args));
        assert!(!permit.matches(&session,"read",&json!({"path":"/b"})));
        assert!(!permit.matches(&session,"exec",&args));
        let mut changed = session.clone(); changed.permission_version += 1;
        assert!(!permit.matches(&changed,"read",&args));
        changed = session.clone(); changed.id = "other-session".into();
        assert!(!permit.matches(&changed,"read",&args));
        changed = session.clone(); changed.knowledge_base_ids.clear();
        assert!(!permit.matches(&changed,"read",&args));
        changed = session; changed.plan_mode = true;
        assert!(!permit.matches(&changed,"read",&args));
    }
}

pub fn check_current(app: &AppHandle, session: &AgentSession) -> Result<(), String> {
    let current = crate::storage::load_persisted_session(app, &session.id)?
        .ok_or_else(|| "持久化会话已失效。".to_owned())?;
    if !permissions::same_authority(session, &current) || current.deleted_at.is_some() {
        return Err("会话权限、范围或 Plan 已变化，请重新开始回合。".into());
    }
    Ok(())
}
