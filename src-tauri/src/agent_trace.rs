use crate::domain::{AgentToolCall, AgentTraceStep};
use crate::storage::{create_id, format_local_datetime};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

/** 前端监听的过程推送事件名，和 UI live 气泡对齐。 */
pub const AGENT_TURN_PROGRESS_EVENT: &str = "agent-turn-progress";

/** 过程区展示的工具结果预览上限，完整 payload 仍按模型预算截断后回填。 */
pub const MAX_TRACE_RESULT_CHARS: usize = 4000;

/** 思考段落写入轨迹前的字符上限，避免单步把会话快照撑爆。 */
pub const MAX_TRACE_THINKING_CHARS: usize = 8000;

/** 工具参数里单个字符串字段的展示上限。 */
const MAX_TRACE_ARG_STRING_CHARS: usize = 500;

/** 不进入用户过程区的基建工具；它们仍保留在 toolCalls 里给审计和旧 UI。 */
const HIDDEN_TOOL_NAMES: [&str; 4] = [
    "skill_context",
    "model_request",
    "activate_skill",
    "local_rule_agent",
];

/** 过程事件载荷，前端按 sessionId 过滤，按 liveMessageId 对齐最终助手消息。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnProgressPayload {
    pub session_id: String,
    pub live_message_id: String,
    pub status: String,
    pub steps: Vec<AgentTraceStep>,
    pub turn_duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/** 一轮 Agent 的过程收集器：内存攒步骤，并尽力向窗口推送增量。 */
pub struct AgentTurnTracer {
    session_id: String,
    live_message_id: String,
    started_at: Instant,
    steps: Vec<AgentTraceStep>,
    status: String,
    content: Option<String>,
    running_started: HashMap<String, Instant>,
}

impl AgentTurnTracer {
    /** 创建过程收集器；live_message_id 必须和最终助手消息 ID 相同。 */
    pub fn new(session_id: impl Into<String>, live_message_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            live_message_id: live_message_id.into(),
            started_at: Instant::now(),
            steps: Vec::new(),
            status: "running".to_owned(),
            content: None,
            running_started: HashMap::new(),
        }
    }

    pub fn live_message_id(&self) -> &str {
        &self.live_message_id
    }

    pub fn steps(&self) -> Vec<AgentTraceStep> {
        self.steps.clone()
    }

    pub fn duration_ms(&self) -> u64 {
        u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /** 回合开始时推一次空过程，让 UI 立刻展开「正在处理」。 */
    pub fn emit_started(&self, app: Option<&AppHandle>) {
        self.emit(app);
    }

    /** 把模型在调用工具前的可见正文记为思考步骤；空白内容直接丢弃。 */
    pub fn push_thinking(&mut self, content: &str, app: Option<&AppHandle>) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return;
        }

        self.steps.push(AgentTraceStep {
            id: create_id("trace"),
            step_type: "thinking".to_owned(),
            timestamp: format_local_datetime(),
            content: Some(truncate_trace_text(trimmed, MAX_TRACE_THINKING_CHARS)),
            name: None,
            status: None,
            summary: None,
            args: None,
            result_preview: None,
            error: None,
            duration_ms: None,
        });
        self.emit(app);
    }

    /** 开始执行用户可见工具；基建工具返回 None 且不发事件。 */
    pub fn begin_tool(
        &mut self,
        name: &str,
        summary: &str,
        args: Value,
        app: Option<&AppHandle>,
    ) -> Option<String> {
        if !is_user_visible_tool(name) {
            return None;
        }

        let step_id = create_id("trace");
        self.running_started.insert(step_id.clone(), Instant::now());
        self.steps.push(AgentTraceStep {
            id: step_id.clone(),
            step_type: "tool".to_owned(),
            timestamp: format_local_datetime(),
            content: None,
            name: Some(name.to_owned()),
            status: Some("running".to_owned()),
            summary: Some(summary.to_owned()),
            args: Some(sanitize_trace_args(args)),
            result_preview: None,
            error: None,
            duration_ms: None,
        });
        self.emit(app);
        Some(step_id)
    }

    /** 用工具最终状态更新对应步骤；隐藏工具或未知 ID 时忽略。 */
    pub fn finish_tool(
        &mut self,
        step_id: Option<&str>,
        status: &str,
        summary: &str,
        result_preview: Option<&str>,
        error: Option<&str>,
        app: Option<&AppHandle>,
    ) {
        let Some(step_id) = step_id else {
            return;
        };
        let duration_ms = self
            .running_started
            .remove(step_id)
            .map(|started_at| u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX));
        let Some(step) = self.steps.iter_mut().find(|step| step.id == step_id) else {
            return;
        };

        step.status = Some(status.to_owned());
        step.summary = Some(summary.to_owned());
        step.result_preview =
            result_preview.map(|value| truncate_trace_text(value, MAX_TRACE_RESULT_CHARS));
        step.error = error
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        step.duration_ms = duration_ms;
        self.emit(app);
    }

    /** 把已经完成的工具清单补进轨迹，供本地兜底等没有逐步执行回调的路径使用。 */
    pub fn ingest_completed_tools(&mut self, tool_calls: &[AgentToolCall]) {
        for tool_call in tool_calls {
            if !is_user_visible_tool(&tool_call.name) {
                continue;
            }

            self.steps.push(completed_tool_step(tool_call));
        }
    }

    pub fn mark_failed(&mut self) {
        self.status = "failed".to_owned();
    }

    /** 结束本轮过程：仍在 running 则记为 completed，并可附带最终回答正文。 */
    pub fn finish(&mut self, content: Option<&str>, app: Option<&AppHandle>) {
        if self.status == "running" {
            self.status = "completed".to_owned();
        }
        if let Some(content) = content.map(str::trim).filter(|value| !value.is_empty()) {
            self.content = Some(content.to_owned());
        }
        self.emit(app);
    }

    fn emit(&self, app: Option<&AppHandle>) {
        let Some(app) = app else {
            return;
        };
        let payload = AgentTurnProgressPayload {
            session_id: self.session_id.clone(),
            live_message_id: self.live_message_id.clone(),
            status: self.status.clone(),
            steps: self.steps.clone(),
            turn_duration_ms: self.duration_ms(),
            content: self.content.clone(),
        };

        if let Err(error) = app.emit(AGENT_TURN_PROGRESS_EVENT, payload) {
            log::debug!(
                target: "agent_runtime",
                "过程事件推送失败：session={} error={}",
                self.session_id,
                error
            );
        }
    }
}

/** 基建工具不进入用户过程区。 */
pub fn is_user_visible_tool(name: &str) -> bool {
    !HIDDEN_TOOL_NAMES.contains(&name)
}

/** 从已完成工具调用生成用户可见轨迹，保持原有顺序。 */
pub fn trace_from_tool_calls(tool_calls: &[AgentToolCall]) -> Vec<AgentTraceStep> {
    tool_calls
        .iter()
        .filter(|tool_call| is_user_visible_tool(&tool_call.name))
        .map(completed_tool_step)
        .collect()
}

/** 把过长文本裁成过程区可展示预览，并保留明确截断标记。 */
pub fn truncate_trace_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_owned();
    }

    let mut truncated: String = value.chars().take(max_chars).collect();
    truncated.push_str("…[已截断]");
    truncated
}

fn completed_tool_step(tool_call: &AgentToolCall) -> AgentTraceStep {
    AgentTraceStep {
        id: create_id("trace"),
        step_type: "tool".to_owned(),
        timestamp: format_local_datetime(),
        content: None,
        name: Some(tool_call.name.clone()),
        status: Some(tool_call.status.clone()),
        summary: Some(tool_call.summary.clone()),
        args: Some(sanitize_trace_args(tool_call.args.clone())),
        result_preview: None,
        error: if tool_call.status == "failed" {
            Some(tool_call.summary.clone())
        } else {
            None
        },
        duration_ms: None,
    }
}

/** 截断参数中的长字符串，避免 propose_file_change 的原文把过程区撑满。 */
fn sanitize_trace_args(value: Value) -> Value {
    match value {
        Value::String(text) => {
            Value::String(truncate_trace_text(&text, MAX_TRACE_ARG_STRING_CHARS))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_trace_args).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, child)| (key, sanitize_trace_args(child)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn completed_call(name: &str, summary: &str, args: Value) -> AgentToolCall {
        AgentToolCall {
            id: create_id("tool"),
            name: name.to_owned(),
            status: "completed".to_owned(),
            summary: summary.to_owned(),
            args,
        }
    }

    /** 基建工具不能出现在用户过程区，避免和 Codex 风格的可读步骤混在一起。 */
    #[test]
    fn hides_internal_tools_from_user_trace() {
        for name in [
            "skill_context",
            "model_request",
            "activate_skill",
            "local_rule_agent",
        ] {
            assert!(!is_user_visible_tool(name), "{name} should stay hidden");
        }

        assert!(is_user_visible_tool("search_notes"));
        assert!(is_user_visible_tool("read_file"));
        assert!(is_user_visible_tool("propose_file_change"));
    }

    /** 空白思考不能生成步骤，否则过程区会出现空段落。 */
    #[test]
    fn skips_empty_thinking_content() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");

        tracer.push_thinking("   \n  ", None);
        tracer.push_thinking("", None);

        assert!(tracer.steps().is_empty());
    }

    /** 思考和工具必须按发生顺序交错，这是排查 Agent 决策的关键。 */
    #[test]
    fn records_thinking_before_tool_in_order() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");

        tracer.push_thinking("先检索相关笔记。", None);
        let step_id = tracer.begin_tool(
            "search_notes",
            "正在调用 search_notes",
            json!({ "query": "本地优先" }),
            None,
        );
        tracer.finish_tool(
            step_id.as_deref(),
            "completed",
            "检索到 2 条笔记",
            Some(r#"{"hits":2}"#),
            None,
            None,
        );

        let steps = tracer.steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step_type, "thinking");
        assert_eq!(steps[0].content.as_deref(), Some("先检索相关笔记。"));
        assert_eq!(steps[1].step_type, "tool");
        assert_eq!(steps[1].name.as_deref(), Some("search_notes"));
        assert_eq!(steps[1].status.as_deref(), Some("completed"));
        assert_eq!(steps[1].summary.as_deref(), Some("检索到 2 条笔记"));
        assert_eq!(steps[1].result_preview.as_deref(), Some(r#"{"hits":2}"#));
    }

    /** 隐藏工具的 begin/finish 必须是空操作，不能污染 live 过程区。 */
    #[test]
    fn hidden_tools_do_not_create_steps() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");
        let step_id = tracer.begin_tool(
            "model_request",
            "正在请求模型",
            json!({ "model": "test" }),
            None,
        );

        assert!(step_id.is_none());
        tracer.finish_tool(
            step_id.as_deref(),
            "completed",
            "完成",
            Some("ok"),
            None,
            None,
        );
        assert!(tracer.steps().is_empty());
    }

    /** 工具结果预览必须截断，完整笔记正文不能原样进入过程轨迹。 */
    #[test]
    fn truncates_long_tool_result_preview() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");
        let step_id =
            tracer.begin_tool("read_file", "正在读取", json!({ "fileId": "note-a" }), None);
        let long_result = "字".repeat(MAX_TRACE_RESULT_CHARS + 80);

        tracer.finish_tool(
            step_id.as_deref(),
            "completed",
            "已读取笔记",
            Some(&long_result),
            None,
            None,
        );

        let steps = tracer.steps();
        let preview = steps[0].result_preview.as_deref().unwrap();
        assert!(preview.chars().count() < long_result.chars().count());
        assert!(preview.ends_with("…[已截断]"));
    }

    /** 失败工具要同时留下 status=failed 和 error，过程区才能默认展开排障。 */
    #[test]
    fn failed_tool_stores_error_and_failed_status() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");
        let step_id = tracer.begin_tool(
            "read_file",
            "正在读取",
            json!({ "fileId": "missing" }),
            None,
        );

        tracer.finish_tool(
            step_id.as_deref(),
            "failed",
            "目标笔记不在 scope 内",
            None,
            Some("目标笔记不在 scope 内"),
            None,
        );

        let step = &tracer.steps()[0];
        assert_eq!(step.status.as_deref(), Some("failed"));
        assert_eq!(step.error.as_deref(), Some("目标笔记不在 scope 内"));
    }

    /** 本地兜底路径没有逐步回调时，仍要从 toolCalls 生成可读轨迹并丢掉基建工具。 */
    #[test]
    fn ingest_completed_tools_filters_internal_calls() {
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");
        tracer.ingest_completed_tools(&[
            completed_call("skill_context", "已注入 Skill 上下文", json!({})),
            completed_call("search_notes", "检索到 1 条笔记", json!({ "query": "abc" })),
            completed_call("model_request", "模型请求完成", json!({})),
        ]);

        let steps = tracer.steps();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name.as_deref(), Some("search_notes"));
        assert_eq!(steps[0].status.as_deref(), Some("completed"));
    }

    /** 参数里的长原文必须截断，过程区只保留排查所需的片段。 */
    #[test]
    fn sanitizes_long_argument_strings() {
        let long_original = "段".repeat(MAX_TRACE_ARG_STRING_CHARS + 40);
        let mut tracer = AgentTurnTracer::new("session-a", "assistant-a");
        tracer.begin_tool(
            "propose_file_change",
            "正在编辑文件",
            json!({ "original": long_original, "count": 1 }),
            None,
        );

        let steps = tracer.steps();
        let args = steps[0].args.as_ref().unwrap();
        let original = args["original"].as_str().unwrap();
        assert!(original.ends_with("…[已截断]"));
        assert_eq!(args["count"], 1);
    }

    /** 从 toolCalls 派生轨迹时也要过滤基建工具，供本地兜底消息直接落库。 */
    #[test]
    fn trace_from_tool_calls_keeps_user_visible_order() {
        let steps = trace_from_tool_calls(&[
            completed_call("skill_context", "上下文", json!({})),
            completed_call("search_notes", "检索", json!({ "query": "q" })),
            completed_call("read_file", "读取", json!({ "fileId": "n1" })),
        ]);

        assert_eq!(
            steps
                .iter()
                .map(|step| step.name.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["search_notes".to_owned(), "read_file".to_owned()]
        );
    }
}
