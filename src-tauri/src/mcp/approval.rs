use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::domain::McpApprovalRequest;
use crate::runtime::AgentCancel;
use crate::storage::create_id;

use super::config::APPROVAL_TIMEOUT_SECONDS;
use super::policy::{arguments_preview, needs_user_approval, CatalogTool};

pub const MCP_APPROVAL_EVENT: &str = "mcp-tool-approval";

/** 回合里正在等用户点允许或拒绝的 MCP 调用。 */
#[derive(Default)]
pub struct ApprovalBoard {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
}

impl ApprovalBoard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decide(&self, approval_id: &str, approved: bool) -> bool {
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(approval_id);
        match sender {
            Some(sender) => sender.send(approved).is_ok(),
            None => false,
        }
    }

    fn begin(&self) -> (String, oneshot::Receiver<bool>) {
        let (sender, receiver) = oneshot::channel();
        let approval_id = create_id("mcp-approval");
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(approval_id.clone(), sender);
        (approval_id, receiver)
    }
}

/**
 * 需要确认时先问用户。拒绝、超时或停止回合都不会调用下游。
 * 允许之后才执行 `call`。
 */
pub async fn confirm_then_call<F, Fut, T>(
    board: &ApprovalBoard,
    app: Option<&AppHandle>,
    session_id: &str,
    security_level: &str,
    tool: &CatalogTool,
    arguments: &Value,
    cancel: &AgentCancel,
    call: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    if !needs_user_approval(security_level, tool.read_only) {
        return call().await;
    }
    if cancel.is_aborted() {
        return Err("用户已停止这个回合。".to_owned());
    }

    let (approval_id, receiver) = board.begin();
    if let Some(app) = app {
        let request = McpApprovalRequest {
            approval_id: approval_id.clone(),
            session_id: session_id.to_owned(),
            server_name: tool.server_name.clone(),
            tool_name: tool.raw_name.clone(),
            arguments_preview: arguments_preview(arguments),
        };
        if let Err(error) = app.emit(MCP_APPROVAL_EVENT, request) {
            board.decide(&approval_id, false);
            return Err(format!("无法发出 MCP 确认：{error}"));
        }
    }

    let decision = tokio::select! {
        decision = receiver => {
            if decision.unwrap_or(false) { ApprovalDecision::Allow } else { ApprovalDecision::Deny }
        }
        _ = tokio::time::sleep(Duration::from_secs(APPROVAL_TIMEOUT_SECONDS)) => ApprovalDecision::Timeout,
        _ = cancel.cancelled() => ApprovalDecision::Abort,
    };
    // 摘掉还没被取走的发送端，避免确认迟到后补调一次。
    board.decide(&approval_id, false);
    match decision {
        ApprovalDecision::Allow => call().await,
        ApprovalDecision::Abort => Err("用户已停止这个回合。".to_owned()),
        ApprovalDecision::Timeout => Err("等待 MCP 确认超时。".to_owned()),
        ApprovalDecision::Deny => Err("用户拒绝了这次 MCP 调用。".to_owned()),
    }
}

enum ApprovalDecision {
    Allow,
    Deny,
    Timeout,
    Abort,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn tool(read_only: bool) -> CatalogTool {
        CatalogTool {
            server_name: "docs".to_owned(),
            raw_name: "write".to_owned(),
            public_name: "mcp__docs__write".to_owned(),
            description: "写入".to_owned(),
            parameters: json!({}),
            read_only,
            exposed: true,
        }
    }

    #[tokio::test]
    async fn denial_does_not_call_downstream() {
        let board = Arc::new(ApprovalBoard::new());
        let called = Arc::new(AtomicBool::new(false));
        let called_flag = called.clone();
        let waiting = board.clone();
        let handle = tokio::spawn(async move {
            confirm_then_call(
                &waiting,
                None,
                "session",
                "advanced",
                &tool(false),
                &json!({ "path": "a" }),
                &AgentCancel::testing_token(),
                || async {
                    called_flag.store(true, Ordering::SeqCst);
                    Ok("called")
                },
            )
            .await
        });
        for _ in 0..50 {
            let ids: Vec<String> = board.pending.lock().unwrap().keys().cloned().collect();
            if let Some(id) = ids.first() {
                board.decide(id, false);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let result = handle.await.expect("join");
        assert!(result.is_err());
        assert!(!called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn readonly_skips_the_prompt() {
        let board = ApprovalBoard::new();
        let result = confirm_then_call(
            &board,
            None,
            "session",
            "advanced",
            &tool(true),
            &json!({}),
            &AgentCancel::testing_token(),
            || async { Ok("ok".to_owned()) },
        )
        .await
        .expect("call");
        assert_eq!(result, "ok");
    }
}
