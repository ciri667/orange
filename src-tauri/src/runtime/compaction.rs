//! 压缩决策与自包含检查点：改写 transcript，不改 UI session.messages。

use super::context::{
    build_model_prompt, conversation_from_model_messages, estimate_model_messages_chars,
    group_transcript_units, project_transcript, render_checkpoint_user_message,
    resolve_compact_reserve_chars, resolve_history_budget_chars, strip_existing_checkpoint,
    ModelPrompt, PackedHistoryStats, CHARS_PER_TOKEN_ESTIMATE, DEFAULT_MODEL_CONTEXT_TOKENS,
};
use crate::domain::{
    AgentContextSummary, AgentContextUsage, AgentSession, AgentSkill, AgentTurnRequest,
    KnowledgeBaseMemory, WorkspaceSnapshot,
};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

/** 同一会话已有进行中的手动整理时返回的稳定错误文案。 */
pub const MANUAL_COMPACTION_ACTIVE_ERROR: &str = "正在整理当前会话上下文，请稍候。";

/** 正在手动整理的会话集合；防止连点触发多份并发摘要请求。 */
static MANUAL_COMPACTIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn lock_manual_compactions() -> MutexGuard<'static, HashSet<String>> {
    MANUAL_COMPACTIONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/**
 * 为一次手动整理登记会话；已有进行中的整理时直接拒绝。
 *
 * 手动整理要发一次模型摘要请求，实测 30–100 秒。没有这道闸门时，用户连点几次就会
 * 并发跑多份摘要，既互相拖慢（日志里出现过 18 份并发），又会用旧快照覆盖彼此结果。
 * 返回的 guard 在离开作用域时自动释放，包括提前 return 和 panic。
 */
pub(super) fn try_begin_manual_compaction(
    session_id: &str,
) -> Result<ManualCompactionGuard, String> {
    let mut active = lock_manual_compactions();
    if !active.insert(session_id.to_owned()) {
        return Err(MANUAL_COMPACTION_ACTIVE_ERROR.to_owned());
    }
    Ok(ManualCompactionGuard {
        session_id: session_id.to_owned(),
    })
}

/** 手动整理的在途登记；Drop 时释放，避免失败路径把会话永久锁死。 */
pub(super) struct ManualCompactionGuard {
    session_id: String,
}

impl Drop for ManualCompactionGuard {
    fn drop(&mut self) {
        lock_manual_compactions().remove(&self.session_id);
    }
}

/** 手动整理保留的最近对话预算：模型窗口的 10%，并夹在 8k–60k 字符之间。 */
pub(super) const MANUAL_COMPACT_TAIL_WINDOW_RATIO: f64 = 0.10;
pub(super) const MANUAL_COMPACT_MIN_TAIL_CHARS: usize = 8_000;
pub(super) const MANUAL_COMPACT_MAX_TAIL_CHARS: usize = 60_000;

/** 一次 transcript 压缩的实际结果，供日志和 UI 说明保留/丢弃了多少历史。 */
#[derive(Clone, Debug, Default)]
pub(super) struct CompactedTranscript {
    pub messages: Vec<Value>,
    /** 压缩后 transcript 的消息数（含检查点）。 */
    pub retained_messages: usize,
    /** 被检查点取代、不再回放的历史消息数。 */
    pub dropped_messages: usize,
}

/** 压缩成功后 transcript = [checkpoint_user, ...legal_tail]。 */
pub(super) fn compact_transcript(
    transcript: &[Value],
    summary: &AgentContextSummary,
    budget_chars: usize,
) -> Vec<Value> {
    compact_transcript_with_stats(transcript, summary, budget_chars).messages
}

/**
 * 压缩 transcript 并返回实际保留/丢弃数量。
 *
 * 保留尾部的预算由调用方决定：自动整理用历史层预算（只在装不下时才触发），
 * 手动整理用更小的 `resolve_manual_compact_tail_budget_chars`。否则大窗口模型
 * （例如 1M token）下预算比历史正文还大，整理只会多一个检查点、一条旧消息都不丢，
 * 用户点了「整理上下文」会觉得按钮完全没反应。
 */
pub(super) fn compact_transcript_with_stats(
    transcript: &[Value],
    summary: &AgentContextSummary,
    budget_chars: usize,
) -> CompactedTranscript {
    let Some(checkpoint) = render_checkpoint_user_message(Some(summary)) else {
        let projected = project_transcript(transcript, Some(summary));
        return CompactedTranscript {
            retained_messages: projected.len(),
            dropped_messages: 0,
            messages: projected,
        };
    };
    let checkpoint_chars = estimate_model_messages_chars(std::slice::from_ref(&checkpoint));
    let available_chars = budget_chars.saturating_sub(checkpoint_chars);
    let body = strip_existing_checkpoint(transcript);
    let projected_body = project_transcript(body, Some(summary));
    let units = group_transcript_units(&projected_body);

    let mut tail_units = Vec::new();
    let mut used_chars = 0usize;
    for unit in units.iter().rev() {
        let unit_chars = estimate_model_messages_chars(unit);
        if !tail_units.is_empty() && used_chars.saturating_add(unit_chars) > available_chars {
            break;
        }
        used_chars = used_chars.saturating_add(unit_chars);
        tail_units.push(unit.clone());
    }
    tail_units.reverse();
    // 保留尾部必须含至少一条 assistant：否则 transcript_is_usable 会判定 transcript 丢了回复，
    // 下一轮改从 session.messages seed，把刚整理掉的历史又整段装回模型上下文。
    if !tail_units.iter().flatten().any(is_assistant_message)
        && projected_body.iter().any(is_assistant_message)
    {
        let mut index = units.len().saturating_sub(tail_units.len());
        while index > 0 {
            index -= 1;
            let unit = units[index].clone();
            let has_assistant = unit.iter().any(is_assistant_message);
            tail_units.insert(0, unit);
            if has_assistant {
                break;
            }
        }
    }

    let retained_tail_messages = tail_units.iter().map(|unit| unit.len()).sum::<usize>();
    let dropped_messages = projected_body.len().saturating_sub(retained_tail_messages);
    let mut compacted = vec![checkpoint];
    compacted.extend(tail_units.into_iter().flatten());

    CompactedTranscript {
        retained_messages: compacted.len(),
        dropped_messages,
        messages: compacted,
    }
}

fn is_assistant_message(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("assistant")
}

/**
 * 手动「整理上下文」保留的最近对话预算。
 *
 * 自动整理只在历史已经装不下时才触发，所以直接用历史层预算；手动整理是用户主动要求
 * 压缩，必须真的丢掉旧历史，因此单独按窗口的 10% 计算，并夹在一个可读的区间里。
 */
pub(super) fn resolve_manual_compact_tail_budget_chars(
    model_context_length: Option<u64>,
) -> usize {
    let window_tokens = model_context_length
        .filter(|tokens| *tokens >= 1_024)
        .unwrap_or(DEFAULT_MODEL_CONTEXT_TOKENS);
    let chars = (window_tokens.saturating_mul(CHARS_PER_TOKEN_ESTIMATE) as f64
        * MANUAL_COMPACT_TAIL_WINDOW_RATIO) as usize;
    chars.clamp(MANUAL_COMPACT_MIN_TAIL_CHARS, MANUAL_COMPACT_MAX_TAIL_CHARS)
}

/** 丢掉尚未成对的尾部 assistant，避免把 overflow 失败条写进检查点。 */
pub(super) fn drop_unpaired_trailing_assistant(messages: &mut Vec<Value>) {
    let Some(last) = messages.last() else {
        return;
    };
    if last.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let has_tool_calls = last
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty());
    if has_tool_calls {
        messages.pop();
    }
}

/** overflow 后重建投影：compact conversation，再拼回唯一 system 与本轮 user。 */
pub(super) fn rebuild_prompt_after_overflow(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    current_user_message_id: &str,
    knowledge_base_memories: &[KnowledgeBaseMemory],
    model_context_length: Option<u64>,
    model_messages: &[Value],
    prefix_len: usize,
) -> ModelPrompt {
    let mut conversation = conversation_from_model_messages(model_messages, prefix_len);
    drop_unpaired_trailing_assistant(&mut conversation);
    let session = &snapshot.sessions[session_index];
    let budget_chars = resolve_history_budget_chars(model_context_length);
    let compacted = if let Some(summary) = session.context_summary.as_ref() {
        compact_transcript(&conversation, summary, budget_chars)
    } else {
        conversation
    };
    build_model_prompt(
        snapshot,
        session_index,
        request,
        available_skills,
        explicit_skills,
        current_user_message_id,
        knowledge_base_memories,
        model_context_length,
        Some(&compacted),
    )
}

/** 旧模型的 usage / overflow 不能拿来压新模型。 */
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn usage_matches_model(usage: Option<&AgentContextUsage>, model_id: &str) -> bool {
    usage.is_some_and(|usage| usage.model_id == model_id)
}

/** 全零 usage 不覆盖上次有效值。 */
pub(super) fn should_record_usage(
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
) -> bool {
    prompt_tokens > 0 || completion_tokens > 0 || total_tokens > 0
}

/** 估算当前上下文 token：优先最近有效 usage，再用字符启发式补上其后增长。 */
pub(super) fn estimate_context_tokens(
    session: &AgentSession,
    estimated_prompt_chars: usize,
    model_id: &str,
) -> Option<u64> {
    let usage = session.context_usage.as_ref()?;
    if usage.model_id != model_id {
        return None;
    }
    let prompt_tokens = if usage.prompt_tokens > 0 {
        usage.prompt_tokens
    } else {
        usage.total_tokens
    };
    if prompt_tokens == 0 {
        return None;
    }
    let recorded_chars_estimate = prompt_tokens.saturating_mul(CHARS_PER_TOKEN_ESTIMATE) as usize;
    if estimated_prompt_chars > recorded_chars_estimate {
        let extra = (estimated_prompt_chars - recorded_chars_estimate) as u64
            / CHARS_PER_TOKEN_ESTIMATE.max(1);
        Some(prompt_tokens.saturating_add(extra))
    } else {
        Some(prompt_tokens)
    }
}

/** 自动 compact 主信号改为窗口预留；消息数等仅在 usage 缺失时后备。 */
pub(super) fn context_summary_auto_decision(
    session: &AgentSession,
    estimated_prompt_chars: usize,
    history_pack: Option<&PackedHistoryStats>,
    model_id: Option<&str>,
    model_context_length: Option<u64>,
    silent_overflow: bool,
) -> super::ContextSummaryAutoDecision {
    let mut reasons = Vec::new();
    let unsummarized_message_count = compact_unsummarized_message_count(session);
    let has_compacted = session
        .context_summary
        .as_ref()
        .and_then(|summary| summary.last_compacted_message_id.as_deref())
        .is_some();
    let usage_stale = session.context_summary.as_ref().is_some_and(|summary| {
        session.context_usage.as_ref().is_some_and(|usage| {
            !usage.recorded_at.is_empty()
                && !summary.updated_at.is_empty()
                && usage.recorded_at < summary.updated_at
        })
    });

    if silent_overflow {
        reasons.push("silentOverflow".to_owned());
    }

    let usage_signal = model_id.and_then(|model_id| {
        if usage_stale {
            return None;
        }
        estimate_context_tokens(session, estimated_prompt_chars, model_id)
    });
    if let Some(estimated_tokens) = usage_signal {
        let window = model_context_length
            .filter(|tokens| *tokens >= 1_024)
            .unwrap_or(0);
        if window > 0 {
            let reserve_tokens = (resolve_compact_reserve_chars(model_context_length) as u64)
                / CHARS_PER_TOKEN_ESTIMATE.max(1);
            if estimated_tokens > window.saturating_sub(reserve_tokens)
                && unsummarized_message_count > 0
            {
                reasons.push("usageOverWindowReserve".to_owned());
            }
        }
    } else {
        if session.messages.len() > super::AUTO_COMPACT_MESSAGE_COUNT_THRESHOLD && !has_compacted {
            reasons.push("messageCountOverThreshold".to_owned());
        }
        if unsummarized_message_count > super::AUTO_COMPACT_UNSUMMARIZED_MESSAGE_THRESHOLD {
            reasons.push("unsummarizedMessagesOverThreshold".to_owned());
        }
        if estimated_prompt_chars > super::AUTO_COMPACT_PROMPT_CHAR_THRESHOLD
            && unsummarized_message_count > 0
        {
            reasons.push("promptCharsOverThreshold".to_owned());
        }
    }

    if let Some(history) = history_pack {
        if history.dropped_session_messages > 0
            && unsummarized_message_count > history.included_session_messages
        {
            reasons.push("unsummarizedHistoryDropped".to_owned());
        }
    }

    super::ContextSummaryAutoDecision {
        should_compact: !reasons.is_empty(),
        reasons,
        estimated_prompt_chars,
        unsummarized_message_count,
    }
}

pub(super) fn compact_unsummarized_message_count(session: &AgentSession) -> usize {
    let Some(last_compacted_message_id) = session
        .context_summary
        .as_ref()
        .and_then(|summary| summary.last_compacted_message_id.as_deref())
    else {
        return session.messages.len();
    };
    session
        .messages
        .iter()
        .position(|message| message.id == last_compacted_message_id)
        .map(|index| session.messages.len().saturating_sub(index + 1))
        .unwrap_or(session.messages.len())
}

/** 把切点之前的对话序列化成带角色标签的短文本，供总结器使用。 */
#[allow(dead_code)]
pub(super) fn serialize_prefix_for_summarizer(prefix: &[Value]) -> String {
    prefix
        .iter()
        .map(|message| {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mut content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if content.chars().count() > super::context::MAX_HISTORY_MESSAGE_CHARS {
                content = super::context::truncate_chars(
                    &content,
                    super::context::MAX_HISTORY_MESSAGE_CHARS,
                );
            }
            format!("[{role}] {content}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/** 从 compact 结果反推被摘要掉的前缀，供总结器输入。 */
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn discarded_prefix<'a>(original: &'a [Value], compacted: &[Value]) -> Vec<Value> {
    let original_body = strip_existing_checkpoint(original);
    let tail = compacted.get(1..).unwrap_or(&[]);
    if tail.is_empty() {
        return original_body.to_vec();
    }
    let tail_start = tail.first();
    original_body
        .iter()
        .take_while(|message| Some(*message) != tail_start)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AgentContextSummary;
    use serde_json::json;

    fn test_summary(goal: &str) -> AgentContextSummary {
        AgentContextSummary {
            version: 1,
            updated_at: "2026-08-26 10:00:00".to_owned(),
            current_goal: Some(goal.to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: None,
            last_compacted_message_id: None,
        }
    }

    #[test]
    fn compact_transcript_starts_with_checkpoint_user_and_keeps_tail_tool() {
        let long_tool_result = format!(
            r#"{{"matches":[{{"title":"旧检索命中","snippet":"{}"}}]}}"#,
            "很长的旧工具结果".repeat(80)
        );
        let tail_tool_result =
            r#"{"matches":[{"title":"最近一次完整检索结果不应被截断","score":0.95}]}"#;
        let transcript = vec![
            json!({ "role": "user", "content": "旧问题" }),
            json!({
                "role": "assistant",
                "content": "先检索旧笔记。",
                "tool_calls": [{
                    "id": "old-call",
                    "type": "function",
                    "function": { "name": "search", "arguments": "{\"query\":\"旧笔记\"}" }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "old-call",
                "content": long_tool_result
            }),
            json!({ "role": "user", "content": "新问题" }),
            json!({
                "role": "assistant",
                "content": "继续检索。",
                "tool_calls": [{
                    "id": "new-call",
                    "type": "function",
                    "function": { "name": "search", "arguments": "{\"query\":\"新问题\"}" }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "new-call",
                "content": tail_tool_result
            }),
        ];

        let compacted = compact_transcript(&transcript, &test_summary("继续整理"), 800);
        let tail_tool = compacted
            .iter()
            .find(|message| message["tool_call_id"] == "new-call")
            .expect("tail tool result should remain intact");

        assert_eq!(compacted[0]["role"], "user");
        assert!(compacted[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("压缩检查点"));
        assert!(compacted[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("继续整理"));
        assert_eq!(tail_tool["content"], tail_tool_result);
        assert!(compacted
            .iter()
            .all(|message| message["tool_call_id"] != "old-call"));
    }

    #[test]
    fn compact_transcript_does_not_start_tail_with_orphan_tool() {
        let transcript = vec![
            json!({ "role": "user", "content": "旧" }),
            json!({
                "role": "assistant",
                "content": "检索",
                "tool_calls": [{
                    "id": "pair-call",
                    "type": "function",
                    "function": { "name": "search", "arguments": "{}" }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "pair-call",
                "content": "x".repeat(200)
            }),
            json!({ "role": "user", "content": "新问题" }),
        ];
        let compacted = compact_transcript(&transcript, &test_summary("目标"), 120);
        assert_eq!(compacted[0]["role"], "user");
        if compacted.len() > 1 {
            assert_ne!(compacted[1]["role"], "tool");
        }
        let roles: Vec<_> = compacted
            .iter()
            .map(|message| message["role"].as_str().unwrap_or(""))
            .collect();
        if roles.contains(&"tool") {
            let tool_index = roles.iter().position(|role| *role == "tool").unwrap();
            assert_eq!(roles[tool_index - 1], "assistant");
        }
    }

    #[test]
    fn drop_unpaired_trailing_assistant_removes_failed_tool_call() {
        let mut messages = vec![
            json!({ "role": "user", "content": "问" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "overflow", "type": "function", "function": { "name": "search", "arguments": "{}" } }]
            }),
        ];
        drop_unpaired_trailing_assistant(&mut messages);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn zero_usage_is_not_recorded() {
        assert!(!should_record_usage(0, 0, 0));
        assert!(should_record_usage(12, 0, 12));
    }

    #[test]
    fn usage_from_other_model_does_not_match() {
        let usage = AgentContextUsage {
            model_id: "old-model".to_owned(),
            prompt_tokens: 100,
            completion_tokens: 10,
            total_tokens: 110,
            recorded_at: "2026-08-26 10:00:00".to_owned(),
            context_length: Some(32_768),
        };
        assert!(!usage_matches_model(Some(&usage), "new-model"));
        assert!(usage_matches_model(Some(&usage), "old-model"));
    }

    #[test]
    fn discarded_prefix_excludes_retained_tail() {
        let original = vec![
            json!({ "role": "user", "content": "旧" }),
            json!({ "role": "assistant", "content": "旧答" }),
            json!({ "role": "user", "content": "新" }),
        ];
        let compacted = vec![
            json!({ "role": "user", "content": "检查点" }),
            json!({ "role": "user", "content": "新" }),
        ];
        let prefix = discarded_prefix(&original, &compacted);
        assert!(prefix.iter().any(|message| message["content"] == "旧"));
        assert!(prefix.iter().all(|message| message["content"] != "新"));
    }

    /**
     * 手动整理必须真的丢掉旧历史。
     *
     * 大窗口模型（1M token）下历史层预算是 204_800 字符，比常见会话正文还大；
     * 若手动整理也用它，`compact_transcript` 只会多一个检查点、一条旧消息都不丢，
     * 用户点「整理上下文」后看不到任何变化。这里用同一份对话对比两种预算。
     */
    #[test]
    fn manual_compact_budget_actually_drops_old_history_on_large_window() {
        let large_window = Some(1_000_000u64);
        let history_budget = resolve_history_budget_chars(large_window);
        let manual_budget = resolve_manual_compact_tail_budget_chars(large_window);
        let transcript = long_transcript(24, 4_000);

        assert!(
            manual_budget < history_budget,
            "手动整理预算必须小于历史层预算，否则无法真正压缩"
        );

        let with_history_budget =
            compact_transcript_with_stats(&transcript, &test_summary("大窗口"), history_budget);
        let with_manual_budget =
            compact_transcript_with_stats(&transcript, &test_summary("大窗口"), manual_budget);

        assert_eq!(
            with_history_budget.dropped_messages, 0,
            "历史层预算装得下全文时不应丢消息，这正是手动整理失效的原因"
        );
        assert!(
            with_manual_budget.dropped_messages > 0,
            "手动整理必须丢弃旧历史"
        );
        assert!(with_manual_budget.retained_messages < transcript.len());
    }

    /** 手动整理保留的尾部必须含 assistant，否则下一轮会判定 transcript 不可用而整段回填历史。 */
    #[test]
    fn manual_compact_keeps_an_assistant_message_in_retained_tail() {
        let transcript = long_transcript(16, 4_000);
        let compacted =
            compact_transcript_with_stats(&transcript, &test_summary("保底"), 8_000);

        assert!(compacted.dropped_messages > 0);
        assert!(
            compacted
                .messages
                .iter()
                .skip(1)
                .any(|message| message["role"] == "assistant"),
            "保留尾部至少要有一条 assistant 回复"
        );
    }

    /** 构造 user/assistant 交替、每条约 `chars_per_message` 字符的对话。 */
    fn long_transcript(turns: usize, chars_per_message: usize) -> Vec<Value> {
        let body = "旧对话正文".repeat(chars_per_message / 5 + 1);

        (0..turns)
            .flat_map(|index| {
                [
                    json!({ "role": "user", "content": format!("第 {index} 问 {body}") }),
                    json!({ "role": "assistant", "content": format!("第 {index} 答 {body}") }),
                ]
            })
            .collect()
    }

    /**
     * 同一会话不能并发整理。
     *
     * 实测连点会并发跑十几份摘要请求，互相拖慢到 30–100 秒；这道闸门让第二次点击
     * 立刻拿到明确错误，而不是排队等一份重复工作。
     */
    #[test]
    fn concurrent_manual_compaction_on_same_session_is_rejected() {
        let session_id = format!("session-compact-{}", uuid::Uuid::new_v4());
        let guard = try_begin_manual_compaction(&session_id).expect("first compaction");
        let second = try_begin_manual_compaction(&session_id);
        assert_eq!(
            second.err().unwrap_or_default(),
            MANUAL_COMPACTION_ACTIVE_ERROR
        );

        // 释放后可以重新整理，失败路径不会把会话永久锁死。
        drop(guard);
        assert!(try_begin_manual_compaction(&session_id).is_ok());
    }

    /** 不同会话的整理互不影响。 */
    #[test]
    fn manual_compaction_guard_is_per_session() {
        let session_a = format!("session-a-{}", uuid::Uuid::new_v4());
        let session_b = format!("session-b-{}", uuid::Uuid::new_v4());
        let guard_a = try_begin_manual_compaction(&session_a).expect("session a");
        assert!(try_begin_manual_compaction(&session_b).is_ok());
        drop(guard_a);
    }
}
