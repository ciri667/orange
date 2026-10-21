//! 主 Agent 的工具轮次预算；浏览器需要连续读取、操作和核对，使用独立上限。

use crate::domain::AgentToolCall;
use serde_json::{json, Value};

/** 普通知识库任务与子 Agent 沿用原有的工具轮次上限。 */
pub(super) const DEFAULT_TOOL_ROUND_LIMIT: usize = 12;
/** 浏览器操作粒度较细，给多字段表单和跨页任务留出足够的执行空间。 */
const BROWSER_TOOL_ROUND_LIMIT: usize = 64;

/** 总结阶段只读取已有工具结果，不再派发工具。 */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelLoopPhase {
    Tools,
    Summary,
}

/** 按含工具调用的模型响应计数；网络重试和最终回答不消耗工具轮次。 */
pub(super) struct ToolRoundBudget {
    used: usize,
    limit: usize,
    summary_requested: bool,
    /** 用户接管属于本轮终止条件，不能继续借用浏览器的扩展预算。 */
    browser_taken_over: bool,
}

impl Default for ToolRoundBudget {
    fn default() -> Self {
        Self {
            used: 0,
            limit: DEFAULT_TOOL_ROUND_LIMIT,
            summary_requested: false,
            browser_taken_over: false,
        }
    }
}

impl ToolRoundBudget {
    /** 耗尽后额外保留一次无工具的总结请求，避免最后一步结果无人读取。 */
    pub(super) fn next_phase(&mut self) -> Option<ModelLoopPhase> {
        if self.summary_requested {
            None
        } else if !self.browser_taken_over && self.used < self.limit {
            Some(ModelLoopPhase::Tools)
        } else {
            self.summary_requested = true;
            Some(ModelLoopPhase::Summary)
        }
    }

    /** 截断参数也占一轮，防止模型持续输出不可执行的工具调用。 */
    pub(super) fn record_tool_round(&mut self) {
        self.used += 1;
    }

    /** 浏览器成功后扩展预算；接管后立即终止，不把终止信号交给模型自行决定。 */
    pub(super) fn observe_call(&mut self, call: &AgentToolCall, payload: &Value) {
        if call.name != "run" || call.args.get("kind").and_then(Value::as_str) != Some("browser") {
            return;
        }
        if payload.pointer("/error/code").and_then(Value::as_str)
            == Some(crate::browser::BROWSER_USER_TAKEOVER)
        {
            self.stop_for_browser_takeover();
        } else if call.status == "completed" {
            self.limit = BROWSER_TOOL_ROUND_LIMIT;
        }
    }

    /** 接管后立即收束整个工具阶段，避免切换工具或派出子 Agent 继续重试。 */
    pub(super) fn stop_for_browser_takeover(&mut self) {
        self.browser_taken_over = true;
    }

    /** 总结和未派发调用共用同一停止原因，不将接管误报成预算耗尽。 */
    pub(super) fn stop_reason(&self) -> Option<&'static str> {
        self.browser_taken_over
            .then_some("用户已接管浏览器，本轮后续工具调用已停止。请等待用户发送下一条消息后继续。")
    }

    /** 临时总结指令只进入该次请求，不写入原 transcript，以免下一轮被误判为仍然受限。 */
    pub(super) fn summary_messages(&self, messages: &[Value]) -> Vec<Value> {
        let mut summary_messages = messages.to_vec();
        let reason = self
            .stop_reason()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("本轮已执行 {} 轮工具调用，工具预算已用完。", self.used));
        summary_messages.push(json!({
            "role": "user",
            "content": format!(
                "【运行时提示】{reason} 现在只根据已有工具结果回答，不再调用任何工具，也不要通过联网读取、MCP 或子 Agent 绕过停止条件。若任务已经完成，给出核验后的结果；若尚未完成，明确说明已完成的部分、未完成的部分，以及下一步需要做什么。不要把未执行或未核验的操作说成成功。"
            )
        }));
        summary_messages
    }

    /** 总结请求失败或仍返回工具调用时保留过程结果，明确告知用户本轮已暂停。 */
    pub(super) fn fallback_content(&self) -> String {
        if let Some(reason) = self.stop_reason() {
            return format!("{reason} 已有执行结果保留在过程区，本轮未能生成结果总结。");
        }
        format!(
            "本轮已执行 {} 轮工具调用，后续操作已暂停，未能生成结果总结。已有执行结果保留在过程区；请发送下一条指令继续未完成的任务。",
            self.used
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser_call(status: &str) -> AgentToolCall {
        AgentToolCall {
            id: "tool-browser".to_owned(),
            name: "run".to_owned(),
            status: status.to_owned(),
            summary: String::new(),
            args: json!({ "kind": "browser", "operation": "page.elements.snapshot" }),
        }
    }

    #[test]
    fn browser_workflow_can_complete_more_than_twelve_tool_rounds() {
        let mut budget = ToolRoundBudget::default();
        // 创建页、导航、多次快照和表单输入，共 30 个依赖前一步结果的模型轮次。
        for _ in 0..30 {
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
            budget.record_tool_round();
            budget.observe_call(&browser_call("completed"), &json!({ "ok": true }));
        }
        assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
    }

    #[test]
    fn repeated_browser_success_does_not_reset_the_hard_limit() {
        let mut budget = ToolRoundBudget::default();
        for _ in 0..BROWSER_TOOL_ROUND_LIMIT {
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
            budget.record_tool_round();
            budget.observe_call(&browser_call("completed"), &json!({ "ok": true }));
        }
        assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Summary));
        assert_eq!(budget.next_phase(), None);
    }

    #[test]
    fn denied_browser_calls_and_skill_runs_keep_the_default_limit() {
        for call in [
            browser_call("failed"),
            AgentToolCall {
                args: json!({ "skillId": "test-skill" }),
                ..browser_call("completed")
            },
        ] {
            let mut budget = ToolRoundBudget::default();
            for _ in 0..DEFAULT_TOOL_ROUND_LIMIT {
                assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
                budget.record_tool_round();
                budget.observe_call(&call, &json!({}));
            }
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Summary));
            assert_eq!(budget.next_phase(), None);
        }
    }

    #[test]
    fn retries_without_tool_responses_do_not_consume_tool_rounds() {
        let mut budget = ToolRoundBudget::default();
        for _ in 0..DEFAULT_TOOL_ROUND_LIMIT + 2 {
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
        }
        assert_eq!(budget.used, 0);
    }

    #[test]
    fn takeover_after_browser_success_immediately_requests_a_tool_free_summary() {
        let mut budget = ToolRoundBudget::default();
        for _ in 0..7 {
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
            budget.record_tool_round();
            budget.observe_call(&browser_call("completed"), &json!({ "ok": true }));
        }
        budget.observe_call(
            &browser_call("failed"),
            &json!({ "ok": false, "error": { "code": "BROWSER_USER_TAKEOVER" } }),
        );
        assert!(budget.stop_reason().unwrap().contains("接管"));
        assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Summary));
        let messages = budget.summary_messages(&[]);
        let notice = messages[0]["content"].as_str().unwrap();
        assert!(notice.contains("接管"));
        assert!(!notice.contains("预算已用完"));
        assert!(budget.fallback_content().contains("接管"));
        let payload =
            super::super::build_chat_completion_payload("test-model", &messages, None, None, true);
        assert!(payload.get("tools").is_none());
        assert_eq!(budget.next_phase(), None);
        // 下一条用户消息构建新预算，不能把接管状态带入下个回合。
        assert_eq!(
            ToolRoundBudget::default().next_phase(),
            Some(ModelLoopPhase::Tools)
        );
    }

    #[test]
    fn takeover_between_calls_stops_dispatch_but_recoverable_errors_do_not() {
        let mut budget = ToolRoundBudget::default();
        for code in ["SNAPSHOT_EXPIRED", "CDP_ERROR", "BROWSER_CONTROL_BUSY"] {
            budget.observe_call(
                &browser_call("failed"),
                &json!({ "ok": false, "error": { "code": code } }),
            );
            assert!(budget.stop_reason().is_none());
            assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Tools));
        }
        budget.stop_for_browser_takeover();
        assert!(budget.stop_reason().is_some());
        assert_eq!(budget.next_phase(), Some(ModelLoopPhase::Summary));
    }

    #[test]
    fn takeover_settles_remaining_browser_web_mcp_and_task_calls_without_execution() {
        let mut budget = ToolRoundBudget::default();
        budget.stop_for_browser_takeover();
        let batch: Vec<Value> = ["run", "read", "mcp__docs__query", "task", "task"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                json!({
                    "id": format!("call-{index}"), "type": "function",
                    "function": { "name": name, "arguments": "{}" }
                })
            })
            .collect();
        let mut messages = vec![json!({ "role": "assistant", "tool_calls": batch })];
        let mut calls = Vec::new();
        let mut tracer = crate::agent_trace::AgentTurnTracer::new("session", "assistant");
        super::super::settle_undispatched_tool_calls(
            batch,
            &mut messages,
            &mut calls,
            &mut tracer,
            None,
            budget.stop_reason().unwrap(),
        );
        assert_eq!(calls.len(), 5);
        assert!(calls
            .iter()
            .all(|call| call.status == "aborted" && call.summary.contains("接管")));
        assert_eq!(
            super::super::filter_incomplete_tool_pairs(&messages).len(),
            6
        );
        for (index, message) in messages.iter().skip(1).enumerate() {
            assert_eq!(message["tool_call_id"], format!("call-{index}"));
        }
    }

    #[test]
    fn summary_reads_the_last_tool_result_without_persisting_the_limit_notice() {
        let budget = ToolRoundBudget::default();
        let transcript = vec![json!({
            "role": "tool", "tool_call_id": "submit", "content": "Received!"
        })];
        let messages = budget.summary_messages(&transcript);
        assert_eq!(messages[0], transcript[0]);
        assert!(messages[1]["content"].as_str().unwrap().contains("未完成"));
        assert_eq!(transcript.len(), 1);
        let payload =
            super::super::build_chat_completion_payload("test-model", &messages, None, None, true);
        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
    }
}
