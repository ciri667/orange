//! 压缩决策与自包含检查点：改写 transcript，不改 UI session.messages。

use super::context::{
    build_model_prompt, conversation_from_model_messages, estimate_model_messages_chars,
    group_transcript_units, project_transcript, render_checkpoint_user_message,
    resolve_compact_reserve_chars, resolve_history_budget_chars, strip_existing_checkpoint,
    ModelPrompt, PackedHistoryStats, CHARS_PER_TOKEN_ESTIMATE,
};
use crate::domain::{
    AgentContextSummary, AgentContextUsage, AgentSession, AgentSkill, AgentTurnRequest,
    KnowledgeBaseMemory, WorkspaceSnapshot,
};
use serde_json::Value;

/** 压缩成功后 transcript = [checkpoint_user, ...legal_tail]。 */
pub(super) fn compact_transcript(
    transcript: &[Value],
    summary: &AgentContextSummary,
    budget_chars: usize,
) -> Vec<Value> {
    let Some(checkpoint) = render_checkpoint_user_message(Some(summary)) else {
        return project_transcript(transcript, Some(summary));
    };
    let checkpoint_chars = estimate_model_messages_chars(std::slice::from_ref(&checkpoint));
    let available_chars = budget_chars.saturating_sub(checkpoint_chars);
    let body = strip_existing_checkpoint(transcript);
    let units = group_transcript_units(&project_transcript(body, Some(summary)));

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

    let mut compacted = vec![checkpoint];
    compacted.extend(tail_units.into_iter().flatten());
    compacted
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
}
