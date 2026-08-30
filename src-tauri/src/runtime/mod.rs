mod compaction;
mod context;
mod dsml;
mod stream;
use compaction::*;
use context::*;
use dsml::*;
use stream::*;

use crate::agent;
use crate::agent_tools::{model_tool_call_name, parse_tool_args, AgentToolContext, ToolRegistry};
use crate::agent_trace::AgentTurnTracer;
use crate::domain::{
    AgentContextSummary, AgentContextTouchedNote, AgentContextUsage, AgentMessage, AgentSession,
    AgentSkill, AgentToolCall, AgentTurnRequest, AgentTurnResult, Citation, KnowledgeBaseMemory,
    LlmProviderConfig, RequestAuditLog, UserSettings, WorkspaceSnapshot,
};
#[cfg(test)]
use crate::domain::{AgentMemoryEntry, ProposedChange};
use crate::logging::{self, AppEventBuilder, AppLogCategory, AppLogLevel};
use crate::model_provider;
use crate::provider_error;
use crate::skills;
use crate::storage::{create_id, format_local_datetime};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** 手动和自动整理上下文时，最多把多少条未总结消息交给总结器。 */
const MAX_RECENT_MESSAGES_FOR_SUMMARY: usize = 24;

/** 超过该消息数后自动触发模型整理；仅在 usage 缺失时作为后备。 */
pub(super) const AUTO_COMPACT_MESSAGE_COUNT_THRESHOLD: usize = 48;

/** 最近未进入 summary 的消息超过该数量时自动整理；仅在 usage 缺失时作为后备。 */
pub(super) const AUTO_COMPACT_UNSUMMARIZED_MESSAGE_THRESHOLD: usize = 20;

/** 估算 prompt 字符数超过该阈值时自动整理；仅在 usage 缺失时作为后备。 */
pub(super) const AUTO_COMPACT_PROMPT_CHAR_THRESHOLD: usize = 48_000;

/** 工具结果回填给模型时的最大 JSON 字符数。 */
const MAX_TOOL_RESULT_CHARS: usize = 9000;

/** 内层工具续跑安全帽；有 tool call 就继续，error/abort/length 在循环内立刻停。 */
const MAX_INNER_TOOL_ROUNDS: usize = 12;

/** 请求审计最多记录的发送片段摘要数量。 */
const MAX_AUDIT_FRAGMENTS: usize = 8;

/** 后端再次限制每轮显式 Skill 数量，避免绕过 UI 传入过多 instructions。 */
const MAX_EXPLICIT_SKILLS_PER_TURN: usize = 3;

/** 云端模型请求超时时间，避免网络卡住后阻塞 Agent turn。 */
const MODEL_HTTP_TIMEOUT_SECONDS: u64 = 60;

/** 本轮显式 Skill 解析结果；skills 已按用户选择顺序去重并校验 enabled。 */
#[derive(Debug)]
struct ExplicitSkillSelection {
    skills: Vec<AgentSkill>,
    requested_count: usize,
    truncated: bool,
}

/** 真实 Agent Runtime 的调度结果，包含可持久化快照和本轮请求审计摘要。 */
pub struct RuntimeTurnResult {
    pub turn_result: AgentTurnResult,
    pub audit_log: RequestAuditLog,
}

/** 可用于整理会话工作记忆的模型配置，统一供自动和手动 compact 复用。 */
struct ContextSummaryModelSelection {
    provider: LlmProviderConfig,
    selected_model_id: String,
    api_key: String,
}

/** 本轮是否触发模型级工作记忆整理的判断结果。 */
pub(super) struct ContextSummaryAutoDecision {
    pub should_compact: bool,
    pub reasons: Vec<String>,
    pub estimated_prompt_chars: usize,
    pub unsummarized_message_count: usize,
}

/** Runtime 内部审计轨迹，用于汇总模型请求次数和实际发送的本地片段摘要。 */
#[derive(Default)]
struct RuntimeAuditTrail {
    model_request_count: usize,
    sent_fragments: Vec<String>,
    context_summary_injected: bool,
    context_summary_prompt_chars: usize,
    context_summary_updated_at: Option<String>,
}

impl RuntimeAuditTrail {
    /** 记录本轮是否把已有工作记忆注入模型请求；只保存长度和更新时间，不保存正文。 */
    fn record_context_summary_injection(&mut self, session: &AgentSession) {
        self.context_summary_prompt_chars =
            render_checkpoint_user_content(session.context_summary.as_ref())
                .map(|prompt| prompt.chars().count())
                .unwrap_or_default();
        self.context_summary_injected = self.context_summary_prompt_chars > 0;
        self.context_summary_updated_at = session
            .context_summary
            .as_ref()
            .map(|summary| summary.updated_at.clone())
            .filter(|updated_at| !updated_at.trim().is_empty());
    }

    /** 记录一次真实模型请求，最终写入 RequestAuditLog 的发送摘要。 */
    fn record_model_request(&mut self) {
        self.model_request_count += 1;
    }

    /** 记录一次工具结果中发送给模型的本地片段摘要。 */
    fn record_sent_fragment(&mut self, fragment: Option<String>) {
        if let Some(fragment) = fragment.filter(|value| !value.trim().is_empty()) {
            self.sent_fragments.push(fragment);
        }
    }

    /** 生成可持久化的发送内容摘要，避免审计日志保存正文。 */
    fn content_summary(&self, base_summary: &str, prompt: &str, session: &AgentSession) -> String {
        let fragment_summary = if self.sent_fragments.is_empty() {
            "发送片段：未发送本地笔记正文".to_owned()
        } else {
            format!(
                "发送片段：{}",
                self.sent_fragments
                    .iter()
                    .take(MAX_AUDIT_FRAGMENTS)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("；")
            )
        };
        let stored_summary_chars = context_summary_rendered_chars(session.context_summary.as_ref());
        let stored_summary_updated_at = session
            .context_summary
            .as_ref()
            .map(|summary| summary.updated_at.as_str())
            .filter(|updated_at| !updated_at.trim().is_empty())
            .unwrap_or("none");
        let injected_summary_updated_at =
            self.context_summary_updated_at.as_deref().unwrap_or("none");

        format!(
            "{}；模型请求 {} 次；输入长度 {} 字符；{}；工作记忆：injected={} injected_chars={} injected_updated_at={} stored={} stored_chars={} stored_updated_at={}",
            base_summary,
            self.model_request_count,
            prompt.chars().count(),
            fragment_summary,
            self.context_summary_injected,
            self.context_summary_prompt_chars,
            injected_summary_updated_at,
            session.context_summary.is_some(),
            stored_summary_chars,
            stored_summary_updated_at
        )
    }
}

/** 解析本轮显式 Skill ID，按选择顺序去重、限制数量，并校验仍存在且已启用。 */
fn resolve_explicit_skills(
    requested_skill_ids: &[String],
    available_skills: &[AgentSkill],
) -> Result<ExplicitSkillSelection, String> {
    let mut seen_skill_ids = HashSet::new();
    let mut normalized_ids = Vec::new();

    for skill_id in requested_skill_ids {
        let skill_id = skill_id.trim();

        if skill_id.is_empty() || !seen_skill_ids.insert(skill_id.to_owned()) {
            continue;
        }

        normalized_ids.push(skill_id.to_owned());
    }

    let truncated = normalized_ids.len() > MAX_EXPLICIT_SKILLS_PER_TURN;
    let normalized_ids = normalized_ids
        .into_iter()
        .take(MAX_EXPLICIT_SKILLS_PER_TURN)
        .collect::<Vec<_>>();
    let mut resolved_skills = Vec::new();

    for skill_id in &normalized_ids {
        let Some(skill) = available_skills.iter().find(|skill| skill.id == *skill_id) else {
            return Err(format!("显式选择的 Skill 不存在或已被移除：{skill_id}"));
        };

        if !skill.enabled {
            return Err(format!(
                "显式选择的 Skill「{}」已禁用，请重新选择已启用 Skill。",
                skill.display_name
            ));
        }

        resolved_skills.push(skill.clone());
    }

    Ok(ExplicitSkillSelection {
        skills: resolved_skills,
        requested_count: requested_skill_ids.len(),
        truncated,
    })
}

/** 统计显式 Skill 来源分布，供运行日志观测，不包含路径或 instructions 正文。 */
fn explicit_skill_source_summary(skills: &[AgentSkill]) -> String {
    let built_in_count = skills
        .iter()
        .filter(|skill| skill.source == skills::BUILT_IN_SKILL_SOURCE)
        .count();
    let custom_count = skills.len().saturating_sub(built_in_count);

    format!("built_in={built_in_count},custom={custom_count}")
}

/** 拼接审计可见 Skill 摘要，显式摘要不包含 instructions 正文。 */
fn format_skill_audit_summary(
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
) -> String {
    if explicit_skills.is_empty() {
        return skills::skill_summary(available_skills);
    }

    format!(
        "{}；{}",
        skills::skill_summary(available_skills),
        skills::explicit_skill_summary(explicit_skills)
    )
}

/** 解析可用于整理会话工作记忆的 provider/model，并读取必要密钥；不记录正文或密钥。 */
fn resolve_context_summary_model_selection(
    settings: &UserSettings,
    session: &AgentSession,
) -> Result<ContextSummaryModelSelection, String> {
    if !settings.model_config.enabled {
        return Err("模型未启用，改用本地确定性整理。".to_owned());
    }

    if settings.privacy_policy != "allow-selected-scope" {
        return Err("隐私策略为仅本地，改用本地确定性整理。".to_owned());
    }

    let selection = model_provider::resolve_model_selection(
        &settings.model_config,
        session.model_provider_id.as_deref(),
        session.model_id.as_deref(),
        None,
        None,
    )
    .map_err(|error| error.to_string())?;
    let provider = selection.provider.clone();
    let selected_model_id = selection.model_id.clone();

    if !provider.supports_tools {
        return Err(format!(
            "Provider「{}」未标记支持工具调用，改用本地确定性整理。",
            provider.name
        ));
    }

    let api_key = if provider.requires_api_key {
        match crate::storage::load_model_api_key(&provider.key_reference) {
            Ok(Some(api_key)) => api_key,
            Ok(None) => {
                return Err(format!(
                    "Provider「{}」未找到模型密钥，改用本地确定性整理。",
                    provider.name
                ))
            }
            Err(error) => return Err(error),
        }
    } else {
        String::new()
    };

    Ok(ContextSummaryModelSelection {
        provider,
        selected_model_id,
        api_key,
    })
}

/** 手动整理指定会话上下文；真实模型不可用时降级为本地确定性整理，并改写 transcript。 */
pub async fn compact_agent_context_summary(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    session_id: &str,
    settings: UserSettings,
) -> Result<WorkspaceSnapshot, String> {
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == session_id)
        .ok_or_else(|| "未找到要整理的 Agent 会话。".to_owned())?;
    let started_at = Instant::now();
    let session_id = snapshot.sessions[session_index].id.clone();

    match resolve_context_summary_model_selection(&settings, &snapshot.sessions[session_index]) {
        Ok(selection) => {
            let client = build_http_client()?;

            update_agent_context_summary_best_effort(
                &client,
                &selection.provider,
                &selection.selected_model_id,
                &selection.api_key,
                &mut snapshot,
                session_index,
                None,
                true,
            )
            .await;
            log::info!(
                target: "agent_runtime",
                "手动整理会话工作记忆完成：session={} duration_ms={} mode=model",
                session_id,
                started_at.elapsed().as_millis()
            );
        }
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "手动整理会话工作记忆使用确定性降级：session={} reason={}",
                session_id,
                model_provider::redact_model_error_text(&error)
            );
            update_agent_context_summary_deterministic(&mut snapshot, session_index, None, true);
            log::info!(
                target: "agent_runtime",
                "手动整理会话工作记忆完成：session={} duration_ms={} mode=deterministic",
                session_id,
                started_at.elapsed().as_millis()
            );
        }
    }

    let model_context_length = snapshot.sessions[session_index]
        .model_id
        .as_deref()
        .and_then(|model_id| {
            settings
                .model_config
                .providers
                .iter()
                .find(|provider| {
                    snapshot.sessions[session_index]
                        .model_provider_id
                        .as_deref()
                        == Some(provider.id.as_str())
                })
                .and_then(|provider| {
                    provider
                        .models
                        .iter()
                        .find(|model| model.id == model_id)
                        .and_then(|model| model.context_length)
                })
        });
    rewrite_stored_transcript(
        app,
        &session_id,
        snapshot.sessions[session_index].context_summary.as_ref(),
        model_context_length,
    );

    Ok(snapshot)
}

/** 运行真实 Agent Runtime；只有用户显式关闭模型或选择本地策略时才回退规则 Agent。 */
pub async fn run_agent_turn(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    settings: UserSettings,
    available_skills: Vec<AgentSkill>,
) -> RuntimeTurnResult {
    let explicit_skill_selection =
        match resolve_explicit_skills(&request.explicit_skill_ids, &available_skills) {
            Ok(selection) => selection,
            Err(error) => {
                log::warn!(
                    target: "agent_runtime",
                    "显式 Skill 解析失败：requested_count={} reason={}",
                    request.explicit_skill_ids.len(),
                    model_provider::redact_model_error_text(&error)
                );
                return skill_activation_error_turn(
                    snapshot,
                    request,
                    &available_skills,
                    &[],
                    &error,
                );
            }
        };
    let explicit_skills = explicit_skill_selection.skills.clone();

    if !explicit_skills.is_empty() {
        log::info!(
            target: "agent_runtime",
            "显式 Skill 解析完成：requested_count={} resolved_count={} truncated={} instruction_chars={} source_summary={}",
            explicit_skill_selection.requested_count,
            explicit_skills.len(),
            explicit_skill_selection.truncated,
            explicit_skills
                .iter()
                .map(|skill| skill.instructions.chars().count())
                .sum::<usize>(),
            explicit_skill_source_summary(&explicit_skills)
        );
    }

    if !settings.model_config.enabled {
        if !explicit_skills.is_empty() {
            return skill_activation_error_turn(
                snapshot,
                request,
                &available_skills,
                &explicit_skills,
                "已显式选择 Skill，但当前模型未启用，无法执行 strict skill turn。请启用真实模型后重试。",
            );
        }

        return fallback_agent_turn(
            app,
            snapshot,
            request,
            &available_skills,
            "模型未启用，使用本地规则 Agent。",
        );
    }

    if settings.privacy_policy != "allow-selected-scope" {
        if !explicit_skills.is_empty() {
            return skill_activation_error_turn(
                snapshot,
                request,
                &available_skills,
                &explicit_skills,
                "已显式选择 Skill，但隐私策略为仅本地，无法把 Skill instructions 发送给真实模型执行。",
            );
        }

        return fallback_agent_turn(
            app,
            snapshot,
            request,
            &available_skills,
            "隐私策略为仅本地，使用本地规则 Agent。",
        );
    }

    // 优先级固定为“本轮 > 会话默认 > 全局默认”；解析失败时返回可见错误，不静默切到其他 provider。
    let session_provider_id = resolve_session_index(&snapshot, &request)
        .ok()
        .and_then(|session_index| snapshot.sessions[session_index].model_provider_id.clone());
    let session_model_id = resolve_session_index(&snapshot, &request)
        .ok()
        .and_then(|session_index| snapshot.sessions[session_index].model_id.clone());
    let selection = match model_provider::resolve_model_selection(
        &settings.model_config,
        session_provider_id.as_deref(),
        session_model_id.as_deref(),
        request.model_provider_id.as_deref(),
        request.model_id.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(error) => {
            return model_error_turn(
                snapshot,
                request,
                None,
                None,
                &available_skills,
                &explicit_skills,
                &error.to_string(),
                None,
            )
        }
    };
    let provider = selection.provider.clone();
    let selected_model_id = selection.model_id.clone();

    if !provider.supports_tools {
        return model_error_turn(
            snapshot,
            request,
            Some(&provider),
            Some(&selected_model_id),
            &available_skills,
            &explicit_skills,
            &format!(
                "Provider「{}」未标记支持工具调用（tool calling），无法用于 Agent Loop。",
                provider.name
            ),
            None,
        );
    }

    let api_key = if provider.requires_api_key {
        match crate::storage::load_model_api_key(&provider.key_reference) {
            Ok(Some(api_key)) => api_key,
            Ok(None) => {
                return model_error_turn(
                    snapshot,
                    request,
                    Some(&provider),
                    Some(&selected_model_id),
                    &available_skills,
                    &explicit_skills,
                    &format!(
                        "Provider「{}」未找到模型密钥。请在设置中保存 API key 后重试。",
                        provider.name
                    ),
                    None,
                )
            }
            Err(error) => {
                return model_error_turn(
                    snapshot,
                    request,
                    Some(&provider),
                    Some(&selected_model_id),
                    &available_skills,
                    &explicit_skills,
                    &error,
                    None,
                )
            }
        }
    } else {
        String::new()
    };

    let live_message_id = create_id("assistant");
    let mut tracer = AgentTurnTracer::new(request.session_id.clone(), live_message_id);
    tracer.emit_started(Some(app));

    match run_model_loop(
        app,
        snapshot.clone(),
        request.clone(),
        available_skills.clone(),
        explicit_skills.clone(),
        provider.clone(),
        selected_model_id.clone(),
        api_key,
        settings.agent_security.clone(),
        &mut tracer,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            tracer.mark_failed();
            let error_text = provider_error::user_facing_provider_error(&error);
            tracer.finish(Some(&error_text), Some(app));
            model_error_turn(
                snapshot,
                request,
                Some(&provider),
                Some(&selected_model_id),
                &available_skills,
                &explicit_skills,
                &error_text,
                Some(&tracer),
            )
        }
    }
}

/** 如果本轮请求显式选择了 providerId/modelId（AgentPanel 的“本轮模型”选择器），把它记为会话默认，
 * 让下次打开该会话时选择器展示“最后一次切换”的模型，而不是每次都回退成全局默认。
 * 未显式选择时保持会话原有设置不变——不能把所有发过消息的会话都动态固定成当前全局默认
 * provider，否则会话会失去“跟随全局默认变化”的语义。 */
fn remember_requested_provider_on_session(
    session: &mut AgentSession,
    requested_provider_id: Option<&str>,
    requested_model_id: Option<&str>,
    resolved_model_id: &str,
) {
    let Some(requested_provider_id) = requested_provider_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if session.model_provider_id.as_deref() == Some(requested_provider_id) {
        if requested_model_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(Some(resolved_model_id))
            == session.model_id.as_deref()
        {
            return;
        }
    }

    session.model_provider_id = Some(requested_provider_id.to_owned());
    session.model_id = Some(
        requested_model_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(resolved_model_id)
            .to_owned(),
    );
    session.updated_at = format_local_datetime();
}

/** 使用 OpenAI-compatible chat completions 跑首版工具调用 loop。 */
async fn run_model_loop(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    available_skills: Vec<AgentSkill>,
    explicit_skills: Vec<AgentSkill>,
    provider: LlmProviderConfig,
    selected_model_id: String,
    api_key: String,
    agent_security: crate::domain::AgentSecuritySettings,
    tracer: &mut AgentTurnTracer,
) -> Result<RuntimeTurnResult, String> {
    let session_index = resolve_session_index(&snapshot, &request)?;

    remember_requested_provider_on_session(
        &mut snapshot.sessions[session_index],
        request.model_provider_id.as_deref(),
        request.model_id.as_deref(),
        &selected_model_id,
    );

    let mut citations = Vec::new();
    let mut audit_trail = RuntimeAuditTrail::default();
    let client = build_http_client()?;
    apply_first_prompt_title(&mut snapshot.sessions[session_index], &request.prompt);
    let current_user_message_id =
        ensure_user_message_for_turn(&mut snapshot.sessions[session_index], &request);
    audit_trail.record_context_summary_injection(&snapshot.sessions[session_index]);
    // 加载当前会话 scope 内已启用的跨会话记忆，失败只写脱敏 warn，不阻塞 Agent 回合。
    let session_knowledge_base_ids = snapshot.sessions[session_index].knowledge_base_ids.clone();
    let kb_memories = load_enabled_session_kb_memories(app, &session_knowledge_base_ids);
    let model_context_length = provider
        .models
        .iter()
        .find(|model| model.id == selected_model_id)
        .and_then(|model| model.context_length);
    let session_id = snapshot.sessions[session_index].id.clone();
    let previous_transcript = match crate::storage::load_agent_session_transcript(app, &session_id)
    {
        Ok(transcript) => transcript,
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "读取会话 transcript 失败，本轮改为从会话消息 seed：session={} error={}",
                session_id,
                model_provider::redact_model_error_text(&error)
            );
            None
        }
    };
    let model_prompt = build_model_prompt(
        &snapshot,
        session_index,
        &request,
        &available_skills,
        &explicit_skills,
        &current_user_message_id,
        &kb_memories,
        model_context_length,
        previous_transcript.as_deref(),
    );
    let history_pack = model_prompt.history.clone();
    let mut prompt_prefix_len = model_prompt.prefix_len;
    let mut model_messages = model_prompt.messages;
    let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
    let tool_registry =
        ToolRegistry::for_session(&snapshot.sessions[session_index], &agent_security);
    let tool_schemas = tool_registry.schemas();
    let mut tool_calls = vec![skill_context_tool_call(&available_skills)];
    tool_calls.extend(activate_skill_tool_calls(
        &explicit_skills,
        "completed",
        None,
    ));
    let mut last_failed_tool_summary: Option<String> = None;
    let mut overflow_retried = false;
    let mut silent_overflow = false;

    tool_calls.push(model_request_tool_call(
        &provider,
        &selected_model_id,
        &endpoint,
        "completed",
    ));

    log::info!(
        target: "agent_runtime",
        "模型 Agent 自主工具选择开始：session={} action={} provider_id={} provider_name={} model={} model_context_length={} enabled_skill_count={} explicit_skill_count={} explicit_instruction_chars={} scope_count={} prompt_chars={}",
        snapshot.sessions[session_index].id,
        request.action,
        provider.id,
        provider.name,
        selected_model_id,
        model_context_length.unwrap_or(0),
        available_skills.iter().filter(|skill| skill.enabled).count(),
        explicit_skills.len(),
        explicit_skills
            .iter()
            .map(|skill| skill.instructions.chars().count())
            .sum::<usize>(),
        snapshot.sessions[session_index].knowledge_base_ids.len(),
        request.prompt.chars().count()
    );

    let mut model_round = 0u32;
    for _ in 0..MAX_INNER_TOOL_ROUNDS {
        audit_trail.record_model_request();
        model_round = model_round.saturating_add(1);
        capture_model_prompt_dump(
            app,
            &session_id,
            &selected_model_id,
            model_context_length,
            model_round,
            &model_messages,
        );
        let response = match send_chat_completion_with_policy(
            &client,
            &provider,
            &selected_model_id,
            &endpoint,
            &api_key,
            &mut model_messages,
            Some(&tool_schemas),
            None,
            &mut |streamed| {
                apply_streamed_assistant_progress(tracer, Some(app), streamed);
            },
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                let status = provider_error::parse_http_status_from_error(&error);
                let kind = provider_error::classify_provider_error(&error, status);
                if kind == provider_error::ProviderErrorKind::Overflow
                    && !overflow_retried
                    && snapshot.sessions[session_index]
                        .model_id
                        .as_deref()
                        .is_none_or(|model_id| model_id == selected_model_id)
                {
                    overflow_retried = true;
                    if snapshot.sessions[session_index].context_summary.is_none() {
                        update_agent_context_summary_deterministic(
                            &mut snapshot,
                            session_index,
                            Some("模型上下文超出窗口，已压缩后重试。"),
                            true,
                        );
                    }
                    let rebuilt = rebuild_prompt_after_overflow(
                        &snapshot,
                        session_index,
                        &request,
                        &available_skills,
                        &explicit_skills,
                        &current_user_message_id,
                        &kb_memories,
                        model_context_length,
                        &model_messages,
                        prompt_prefix_len,
                    );
                    prompt_prefix_len = rebuilt.prefix_len;
                    model_messages = rebuilt.messages;
                    log::info!(
                        target: "agent_runtime",
                        "超窗已 compact 并重建投影，准备重试一次：session={}",
                        session_id
                    );
                    continue;
                }
                return Err(provider_error::user_facing_provider_error(&error));
            }
        };
        record_context_usage(
            &mut snapshot.sessions[session_index],
            &selected_model_id,
            &response,
            model_context_length,
        );
        if provider_error::is_silent_overflow(&response, model_context_length) {
            silent_overflow = true;
        }
        let finish_reason = provider_error::parse_finish_reason(&response);
        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| "模型响应缺少 message。".to_owned())?;
        let extracted_tool_calls = extract_tool_calls_from_message(&message);
        let model_tool_calls = extracted_tool_calls.tool_calls;
        log::debug!(
            target: "agent_runtime",
            "模型返回工具调用：session={} tool_call_count={} dsml_visible_chars={} finish_reason={:?}",
            snapshot.sessions[session_index].id,
            model_tool_calls.len(),
            extracted_tool_calls.visible_content.chars().count(),
            finish_reason
        );

        if provider_error::is_length_stop(finish_reason.as_deref()) && !model_tool_calls.is_empty()
        {
            log::info!(
                target: "agent_runtime",
                "finish_reason=length，整批拒绝 {} 个 tool call",
                model_tool_calls.len()
            );
            model_messages.push(normalize_assistant_tool_message(
                message,
                &model_tool_calls,
                &extracted_tool_calls.visible_content,
            ));
            if !tracer.last_step_is_thinking() {
                tracer.push_thinking(&extracted_tool_calls.visible_content, Some(app));
            }
            let failure_text = "工具参数可能被截断，本批调用未执行。请用完整参数重新发送。";
            for model_tool_call in &model_tool_calls {
                let failed_call = AgentToolCall {
                    id: create_id("tool"),
                    name: model_tool_call_name(model_tool_call),
                    status: "failed".to_owned(),
                    summary: failure_text.to_owned(),
                    args: parse_tool_args(model_tool_call),
                };
                tool_calls.push(failed_call);
                model_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": model_tool_call.get("id").and_then(Value::as_str).unwrap_or("tool-call"),
                    "content": json!({ "success": false, "error": failure_text }).to_string()
                }));
            }
            last_failed_tool_summary = Some(failure_text.to_owned());
            continue;
        }

        if provider_error::is_length_stop(finish_reason.as_deref())
            && model_tool_calls.is_empty()
            && extracted_tool_calls.visible_content.is_empty()
            && !overflow_retried
        {
            overflow_retried = true;
            if snapshot.sessions[session_index].context_summary.is_none() {
                update_agent_context_summary_deterministic(
                    &mut snapshot,
                    session_index,
                    Some("模型输出因长度限制被截断，已压缩后重试。"),
                    true,
                );
            }
            let rebuilt = rebuild_prompt_after_overflow(
                &snapshot,
                session_index,
                &request,
                &available_skills,
                &explicit_skills,
                &current_user_message_id,
                &kb_memories,
                model_context_length,
                &model_messages,
                prompt_prefix_len,
            );
            prompt_prefix_len = rebuilt.prefix_len;
            model_messages = rebuilt.messages;
            continue;
        }

        if model_tool_calls.is_empty() {
            let content = if extracted_tool_calls.visible_content.is_empty() {
                if provider_error::is_length_stop(finish_reason.as_deref()) {
                    "模型输出因长度限制被截断，没有完整回复。请用更短的下一步继续。".to_owned()
                } else {
                    "模型未返回可展示内容。".to_owned()
                }
            } else if provider_error::is_length_stop(finish_reason.as_deref()) {
                format!(
                    "{}\n\n（输出因长度限制被截断）",
                    extracted_tool_calls.visible_content
                )
            } else {
                extracted_tool_calls.visible_content
            };
            tracer.finish(Some(&content), Some(app));
            // 最终回复必须进入 transcript，否则下一轮投影只有 user，模型看不到自己说过什么。
            model_messages.push(json!({
                "role": "assistant",
                "content": content.clone()
            }));

            push_assistant_message(
                &mut snapshot,
                session_index,
                &request.action,
                content,
                citations,
                tool_calls,
                tracer,
            );
            let compacted = update_agent_context_summary_after_turn(
                &client,
                &provider,
                &selected_model_id,
                &api_key,
                &mut snapshot,
                session_index,
                estimate_model_messages_chars(&model_messages),
                last_failed_tool_summary.as_deref(),
                Some(&history_pack),
                model_context_length,
                silent_overflow,
            )
            .await;
            persist_turn_transcript(
                app,
                &session_id,
                &model_messages,
                prompt_prefix_len,
                snapshot.sessions[session_index].context_summary.as_ref(),
                model_context_length,
                compacted,
            );
            let audit_log = build_audit_log(
                "model_turn",
                &snapshot,
                session_index,
                &request.prompt,
                &format!(
                    "OpenAI-compatible 模型请求；{}",
                    format_skill_audit_summary(&available_skills, &explicit_skills)
                ),
                &audit_trail,
            );

            return Ok(RuntimeTurnResult {
                turn_result: AgentTurnResult { snapshot },
                audit_log,
            });
        }

        model_messages.push(normalize_assistant_tool_message(
            message,
            &model_tool_calls,
            &extracted_tool_calls.visible_content,
        ));

        if !tracer.last_step_is_thinking() {
            tracer.push_thinking(&extracted_tool_calls.visible_content, Some(app));
        }

        for model_tool_call in model_tool_calls {
            let tool_name = model_tool_call_name(&model_tool_call);
            let tool_args = parse_tool_args(&model_tool_call);
            let trace_step_id = tracer.begin_tool(
                &tool_name,
                &format!("正在调用 {tool_name}"),
                tool_args,
                Some(app),
            );
            let tool_outcome = {
                let mut tool_context = AgentToolContext {
                    app: Some(app),
                    snapshot: &mut snapshot,
                    session_index,
                    request: &request,
                };

                tool_registry.execute_model_tool_call(&mut tool_context, &model_tool_call)
            };
            let mut tool_result_text = truncate_tool_result_for_model(
                &tool_outcome.payload.to_string(),
                MAX_TOOL_RESULT_CHARS,
            );
            if tool_outcome.call.name == "run" && tool_outcome.call.status == "completed" {
                let pending_request = snapshot.sessions[session_index].pending_execution.clone();
                if let Some(pending_request) = pending_request.filter(|pending_request| {
                    crate::skill_execution::can_auto_execute(
                        &snapshot.sessions[session_index],
                        pending_request,
                        &agent_security,
                    )
                }) {
                    // 自动批准仍先持久化结构化请求，审批执行器只从 SQLite 重载可信载荷。
                    crate::storage::save_snapshot_session(app, &snapshot, &request.session_id)?;
                    let execution_app = app.clone();
                    let execution_snapshot = snapshot.clone();
                    snapshot = tauri::async_runtime::spawn_blocking(move || {
                        crate::skill_execution::approve_and_execute(
                            &execution_app,
                            execution_snapshot,
                        )
                    })
                    .await
                    .map_err(|error| format!("完全级别 Skill 执行任务失败：{error}"))??;
                    tool_result_text = truncate_chars(
                        &json!({
                            "executionId": pending_request.id,
                            "status": "completed",
                            "autoApproved": true,
                            "changeCount": snapshot.sessions[session_index]
                                .pending_change_set
                                .as_ref()
                                .map(|change_set| change_set.operations.len())
                                .unwrap_or_default()
                        })
                        .to_string(),
                        MAX_TOOL_RESULT_CHARS,
                    );
                }
            }
            // 完全级别下，Agent 直接产出的变更集在校验通过后自动应用；
            // 校验失败（受保护目录、既有文件冲突等）会暂停自动应用，保留 pending 让用户处理。
            if tool_outcome.call.status == "completed"
                && crate::skill_execution::can_auto_apply_agent_change_set(
                    &snapshot.sessions[session_index],
                    &agent_security,
                )
            {
                crate::storage::save_snapshot_session(app, &snapshot, &request.session_id)?;
                let apply_app = app.clone();
                let apply_snapshot = snapshot.clone();
                let auto_result = tauri::async_runtime::spawn_blocking(move || {
                    crate::skill_execution::apply_agent_change_set(&apply_app, apply_snapshot)
                })
                .await
                .map_err(|error| format!("自主模式 Agent 变更集应用任务失败：{error}"))?;
                match auto_result {
                    Ok(applied_snapshot) => {
                        snapshot = applied_snapshot;
                        let operation_count = snapshot.sessions[session_index]
                            .pending_change_set
                            .as_ref()
                            .map(|change_set| change_set.operations.len())
                            .unwrap_or_default();
                        tool_result_text = truncate_chars(
                            &json!({
                                "status": "applied",
                                "autoApproved": true,
                                "operationCount": operation_count
                            })
                            .to_string(),
                            MAX_TOOL_RESULT_CHARS,
                        );
                    }
                    Err(error) => {
                        // 校验未通过：保留 pending，告知模型需等待用户处理，避免它反复重试同一操作。
                        tool_result_text = truncate_chars(
                            &json!({
                                "status": "pending_review",
                                "autoApproved": false,
                                "reason": error
                            })
                            .to_string(),
                            MAX_TOOL_RESULT_CHARS,
                        );
                    }
                }
            }
            // 完全级别下，Agent 主写入路径（改写/新建文档）校验通过后自动落盘；
            // 这是放手策略的核心，不只覆盖 Skill 或建文件夹。
            if tool_outcome.call.status == "completed"
                && crate::agent_writes::can_auto_apply_pending_change(
                    &snapshot.sessions[session_index],
                    &agent_security,
                )
            {
                crate::storage::save_snapshot_session(app, &snapshot, &request.session_id)?;
                let apply_app = app.clone();
                let apply_snapshot = snapshot.clone();
                let auto_result = tauri::async_runtime::spawn_blocking(move || {
                    crate::agent_writes::apply_pending_change(&apply_app, apply_snapshot)
                })
                .await
                .map_err(|error| format!("自主模式 Agent 写入任务失败：{error}"))?;
                match auto_result {
                    Ok(applied_snapshot) => {
                        snapshot = applied_snapshot;
                        update_agent_context_summary_deterministic(
                            &mut snapshot,
                            session_index,
                            None,
                            false,
                        );
                        crate::storage::save_snapshot_session(app, &snapshot, &request.session_id)?;
                        tool_result_text = truncate_chars(
                            &json!({
                                "status": "applied",
                                "autoApproved": true
                            })
                            .to_string(),
                            MAX_TOOL_RESULT_CHARS,
                        );
                    }
                    Err(error) => {
                        tool_result_text = truncate_chars(
                            &json!({
                                "status": "pending_review",
                                "autoApproved": false,
                                "reason": error
                            })
                            .to_string(),
                            MAX_TOOL_RESULT_CHARS,
                        );
                    }
                }
            }
            log::debug!(
                target: "agent_runtime",
                "工具调用完成：session={} tool={} status={}",
                snapshot.sessions[session_index].id,
                tool_outcome.call.name,
                tool_outcome.call.status
            );

            audit_trail.record_sent_fragment(tool_outcome.audit_fragment);
            citations.extend(tool_outcome.citations);
            let tool_error = if tool_outcome.call.status == "failed" {
                Some(tool_outcome.call.summary.clone())
            } else {
                None
            };
            tracer.finish_tool(
                trace_step_id.as_deref(),
                &tool_outcome.call.status,
                &tool_outcome.call.summary,
                Some(&tool_result_text),
                tool_error.as_deref(),
                Some(app),
            );
            if let Some(tool_error) = tool_error {
                last_failed_tool_summary = Some(tool_error);
            }
            tool_calls.push(tool_outcome.call);
            model_messages.push(json!({
                "role": "tool",
                "tool_call_id": model_tool_call.get("id").and_then(Value::as_str).unwrap_or("tool-call"),
                "content": tool_result_text
            }));
        }
    }

    let content = reconcile_final_content_with_tool_status(
        "已达到本轮工具步数上限。请根据已有结果继续，或再发一条指令。".to_owned(),
        last_failed_tool_summary.as_deref(),
    );
    tracer.finish(Some(&content), Some(app));
    model_messages.push(json!({
        "role": "assistant",
        "content": content.clone()
    }));

    push_assistant_message(
        &mut snapshot,
        session_index,
        &request.action,
        content,
        citations,
        tool_calls,
        tracer,
    );
    let compacted = update_agent_context_summary_after_turn(
        &client,
        &provider,
        &selected_model_id,
        &api_key,
        &mut snapshot,
        session_index,
        estimate_model_messages_chars(&model_messages),
        last_failed_tool_summary.as_deref(),
        Some(&history_pack),
        model_context_length,
        silent_overflow,
    )
    .await;
    persist_turn_transcript(
        app,
        &session_id,
        &model_messages,
        prompt_prefix_len,
        snapshot.sessions[session_index].context_summary.as_ref(),
        model_context_length,
        compacted,
    );
    let audit_log = build_audit_log(
        "model_turn",
        &snapshot,
        session_index,
        &request.prompt,
        &format!(
            "OpenAI-compatible 工具 loop；{}",
            format_skill_audit_summary(&available_skills, &explicit_skills)
        ),
        &audit_trail,
    );

    Ok(RuntimeTurnResult {
        turn_result: AgentTurnResult { snapshot },
        audit_log,
    })
}

/** 工具失败时覆盖模型的成功话术，避免 UI 同时展示 failed 轨迹和“已生成”。 */
fn reconcile_final_content_with_tool_status(
    content: String,
    failed_tool_summary: Option<&str>,
) -> String {
    let Some(failed_tool_summary) = failed_tool_summary else {
        return content;
    };
    let success_markers = [
        "✅",
        "已生成",
        "生成完成",
        "变更已生成",
        "已完成",
        "成功",
        "已经生成",
    ];

    if success_markers
        .iter()
        .any(|marker| content.contains(marker))
    {
        return format!(
            "这次变更没有生成成功：{failed_tool_summary}\n\n我需要重新定位更精确的片段后再生成待确认 diff。"
        );
    }

    content
}

/** 构建带超时的 HTTP client，避免模型 provider 无响应时卡住 Agent turn。 */
fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(MODEL_HTTP_TIMEOUT_SECONDS))
        .build()
        .map_err(|error| format!("无法创建模型 HTTP client：{error}"))
}

fn record_context_usage(
    session: &mut AgentSession,
    model_id: &str,
    response: &Value,
    model_context_length: Option<u64>,
) {
    let Some(usage) = provider_error::parse_completion_usage(response) else {
        return;
    };
    if !should_record_usage(
        usage.prompt_tokens,
        usage.completion_tokens,
        usage.total_tokens,
    ) {
        return;
    }
    session.context_usage = Some(AgentContextUsage {
        model_id: model_id.to_owned(),
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        recorded_at: format_local_datetime(),
        context_length: model_context_length.filter(|tokens| *tokens >= 1_024),
    });
}

/** 把本轮即将发给模型的 messages 落到日志目录，并写一条不含正文的大纲。 */
fn capture_model_prompt_dump(
    app: &AppHandle,
    session_id: &str,
    model_id: &str,
    model_context_length: Option<u64>,
    round: u32,
    messages: &[Value],
) {
    let dump = build_prompt_dump(
        session_id,
        model_id,
        model_context_length,
        round,
        "turn",
        messages,
        &format_local_datetime(),
    );
    log::info!(
        target: "agent_runtime",
        "发给模型的上下文：session={} model={} window={} round={} messages={} total_chars={} outline={}",
        session_id,
        model_id,
        model_context_length.unwrap_or(0),
        round,
        dump.messages.len(),
        dump.total_chars,
        dump.outline
    );
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "model_prompt_dump",
            "completed",
            format!(
                "发给模型的上下文：round={} messages={} total_chars={} outline={}",
                round,
                dump.messages.len(),
                dump.total_chars,
                dump.outline
            ),
        )
        .session_id(session_id.to_owned())
        .metadata(json!({
            "modelId": model_id,
            "modelContextLength": model_context_length.unwrap_or(0),
            "round": round,
            "messageCount": dump.messages.len(),
            "totalChars": dump.total_chars,
        })),
    );
    match logging::persist_agent_prompt_dump(app, &dump) {
        Ok(path) => {
            log::debug!(
                target: "agent_runtime",
                "模型上下文转储已写入：session={} path={}",
                session_id,
                path.display()
            );
        }
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "保存模型上下文转储失败：session={} error={}",
                session_id,
                model_provider::redact_model_error_text(&error)
            );
        }
    }
}

/** 把本轮实际发给模型的对话 transcript 落盘；超预算只通过 compact_transcript 改写。 */
fn persist_turn_transcript(
    app: &AppHandle,
    session_id: &str,
    model_messages: &[Value],
    prefix_len: usize,
    context_summary: Option<&AgentContextSummary>,
    model_context_length: Option<u64>,
    rewrite_compacted: bool,
) {
    let mut conversation = conversation_from_model_messages(model_messages, prefix_len);
    if rewrite_compacted {
        if let Some(summary) = context_summary {
            conversation = compact_transcript(
                &conversation,
                summary,
                resolve_history_budget_chars(model_context_length),
            );
        }
    }

    if let Err(error) =
        crate::storage::save_agent_session_transcript(app, session_id, &conversation)
    {
        log::warn!(
            target: "agent_runtime",
            "保存会话 transcript 失败：session={} error={}",
            session_id,
            model_provider::redact_model_error_text(&error)
        );
    }
}

/** 手动/自动 compact 成功后把已落盘 transcript 改写成检查点 + legal tail。 */
fn rewrite_stored_transcript(
    app: &AppHandle,
    session_id: &str,
    context_summary: Option<&AgentContextSummary>,
    model_context_length: Option<u64>,
) {
    let Some(summary) = context_summary else {
        return;
    };
    let transcript = match crate::storage::load_agent_session_transcript(app, session_id) {
        Ok(Some(transcript)) if !transcript.is_empty() => transcript,
        Ok(_) => return,
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "读取会话 transcript 失败，跳过检查点改写：session={} error={}",
                session_id,
                model_provider::redact_model_error_text(&error)
            );
            return;
        }
    };
    let compacted = compact_transcript(
        &transcript,
        summary,
        resolve_history_budget_chars(model_context_length),
    );
    if let Err(error) = crate::storage::save_agent_session_transcript(app, session_id, &compacted) {
        log::warn!(
            target: "agent_runtime",
            "保存压缩后 transcript 失败：session={} error={}",
            session_id,
            model_provider::redact_model_error_text(&error)
        );
    }
}

/** 读取当前会话 scope 内已启用的跨会话记忆；读取失败只写脱敏 warn，返回空集合不阻塞 Agent 回合。 */
fn load_enabled_session_kb_memories(
    app: &AppHandle,
    knowledge_base_ids: &[String],
) -> Vec<KnowledgeBaseMemory> {
    let mut memories = Vec::new();
    for knowledge_base_id in knowledge_base_ids {
        match crate::storage::load_knowledge_base_memory(app, knowledge_base_id) {
            Ok(Some(memory)) if memory.enabled && !memory.entries.is_empty() => {
                memories.push(memory);
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!(
                    target: "agent_memory",
                    "读取跨会话记忆失败，已跳过该知识库：knowledge_base_id_chars={} error={}",
                    knowledge_base_id.chars().count(),
                    crate::logging::sanitize_log_text(&error)
                );
            }
        }
    }
    memories
}

/** 发送模型请求：配额不重试，超窗 compact 后最多一次，429/5xx 指数退避。 */
async fn send_chat_completion_with_policy(
    client: &Client,
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    api_key: &str,
    messages: &mut Vec<Value>,
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
    on_progress: &mut impl FnMut(&StreamedAssistant),
) -> Result<Value, String> {
    let mut delay = Duration::from_millis(400);
    let mut retryable_attempts = 0usize;
    loop {
        match send_chat_completion_logged_stream(
            client,
            provider,
            model_id,
            endpoint,
            api_key,
            messages,
            tool_schemas,
            response_format,
            on_progress,
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(error) => {
                let status = provider_error::parse_http_status_from_error(&error);
                let kind = provider_error::classify_provider_error(&error, status);
                log::info!(
                    target: "agent_runtime",
                    "模型请求失败分类：kind={:?} http_status={:?} retryable_attempts={}",
                    kind,
                    status,
                    retryable_attempts
                );
                match kind {
                    provider_error::ProviderErrorKind::Abort
                    | provider_error::ProviderErrorKind::Quota => {
                        return Err(provider_error::user_facing_provider_error(&error));
                    }
                    provider_error::ProviderErrorKind::Overflow => {
                        return Err(error);
                    }
                    provider_error::ProviderErrorKind::Retryable if retryable_attempts < 3 => {
                        retryable_attempts += 1;
                        std::thread::sleep(delay);
                        delay = delay.saturating_mul(2);
                        continue;
                    }
                    _ => return Err(provider_error::user_facing_provider_error(&error)),
                }
            }
        }
    }
}

/** 发送一次流式 chat completions，并记录 providerId/model/status/耗时/endpointHost；错误统一脱敏。 */
async fn send_chat_completion_logged_stream(
    client: &Client,
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    api_key: &str,
    messages: &[Value],
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
    on_progress: &mut impl FnMut(&StreamedAssistant),
) -> Result<Value, String> {
    let started_at = Instant::now();
    let result = send_chat_completion_stream(
        client,
        endpoint,
        api_key,
        model_id,
        messages,
        tool_schemas,
        response_format,
        on_progress,
    )
    .await;

    match &result {
        Ok(_) => log_model_request_event(
            provider,
            model_id,
            endpoint,
            "completed",
            started_at.elapsed(),
            None,
        ),
        Err(error) => log_model_request_event(
            provider,
            model_id,
            endpoint,
            "failed",
            started_at.elapsed(),
            Some(error),
        ),
    }

    result
}

/** 发送一次 chat completions 请求并记录 providerId/model/status/耗时/endpointHost；错误统一脱敏。 */
async fn send_chat_completion_logged(
    client: &Client,
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    api_key: &str,
    messages: &[Value],
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
) -> Result<Value, String> {
    let started_at = Instant::now();
    let result = send_chat_completion(
        client,
        endpoint,
        api_key,
        model_id,
        messages,
        tool_schemas,
        response_format,
    )
    .await;

    match &result {
        Ok(_) => log_model_request_event(
            provider,
            model_id,
            endpoint,
            "completed",
            started_at.elapsed(),
            None,
        ),
        Err(error) => log_model_request_event(
            provider,
            model_id,
            endpoint,
            "failed",
            started_at.elapsed(),
            Some(error),
        ),
    }

    result
}

/** 记录一次模型请求的分级日志；日志只包含 providerId/providerName/model/status/耗时/endpointHost，不含密钥或正文。 */
fn log_model_request_event(
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    status: &str,
    duration: Duration,
    error: Option<&str>,
) {
    let endpoint_host = model_provider::endpoint_host(endpoint);

    match error {
        Some(error) => log::warn!(
            target: "agent_runtime",
            "模型请求失败：provider_id={} provider_name={} model={} status={} duration_ms={} endpoint_host={} error={}",
            provider.id,
            provider.name,
            model_id,
            status,
            duration.as_millis(),
            endpoint_host,
            model_provider::redact_model_error_text(error)
        ),
        None => log::info!(
            target: "agent_runtime",
            "模型请求完成：provider_id={} provider_name={} model={} status={} duration_ms={} endpoint_host={}",
            provider.id,
            provider.name,
            model_id,
            status,
            duration.as_millis(),
            endpoint_host
        ),
    }
}

/** 构造 chat completions JSON；工具 schema 和 json_schema 互斥出现在不同请求里。 */
fn build_chat_completion_payload(
    model: &str,
    messages: &[Value],
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
    stream: bool,
) -> Value {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "temperature": 0.2
    });

    if let Some(tool_schemas) = tool_schemas {
        // 工具 schema 来自当前会话的动态 ToolRegistry，基础级别不会暴露高权限工具。
        payload["tools"] = tool_schemas.clone();
        payload["tool_choice"] = json!("auto");
    }

    if let Some(response_format) = response_format {
        payload["response_format"] = response_format.clone();
    }

    if stream {
        payload["stream"] = json!(true);
        // OpenAI 兼容流默认不带 usage；显式打开后才能记账窗口占用。
        payload["stream_options"] = json!({ "include_usage": true });
    }

    payload
}

/** 工作记忆请求使用 json_schema，减少模型把 JSON 包进 Markdown fence 的情况。 */
fn context_summary_response_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "agent_context_summary",
            "strict": true,
            "schema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "version": { "type": "integer" },
                    "updatedAt": { "type": "string" },
                    "currentGoal": { "type": ["string", "null"] },
                    "userConstraints": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "decisions": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "completedWork": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "pendingTasks": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "touchedNotes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "id": { "type": "string" },
                                "title": { "type": "string" },
                                "reason": { "type": "string" }
                            },
                            "required": ["id", "title", "reason"]
                        }
                    },
                    "pendingChangeSummary": { "type": ["string", "null"] },
                    "openQuestions": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "lastSummarizedMessageId": { "type": ["string", "null"] },
                    "lastCompactedMessageId": { "type": ["string", "null"] }
                },
                "required": [
                    "version",
                    "updatedAt",
                    "currentGoal",
                    "userConstraints",
                    "decisions",
                    "completedWork",
                    "pendingTasks",
                    "touchedNotes",
                    "pendingChangeSummary",
                    "openQuestions",
                    "lastSummarizedMessageId",
                    "lastCompactedMessageId"
                ]
            }
        }
    })
}

/** 发送一次 chat completions 请求，可选择是否携带工具定义；无 key 的本地免鉴权 provider 不附带 Authorization。 */
async fn send_chat_completion(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    messages: &[Value],
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
) -> Result<Value, String> {
    let payload =
        build_chat_completion_payload(model, messages, tool_schemas, response_format, false);

    let mut request_builder = client.post(endpoint).json(&payload);

    if !api_key.trim().is_empty() {
        request_builder = request_builder.bearer_auth(api_key);
    }

    let response = request_builder.send().await.map_err(|error| {
        model_provider::redact_model_error_text(&format!("无法发送模型请求：{error}"))
    })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("无法读取模型响应：{error}"))?;

    if !status.is_success() {
        return Err(model_provider::redact_model_error_text(&format!(
            "模型请求失败：HTTP {status} {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|error| format!("无法解析模型响应：{error}"))
}

/** 发送流式 chat completions；SSE 边读边回调，provider 若返回完整 JSON 则只回调一次。 */
async fn send_chat_completion_stream(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    messages: &[Value],
    tool_schemas: Option<&Value>,
    response_format: Option<&Value>,
    on_progress: &mut impl FnMut(&StreamedAssistant),
) -> Result<Value, String> {
    let mut payload =
        build_chat_completion_payload(model, messages, tool_schemas, response_format, true);

    loop {
        let mut request_builder = client.post(endpoint).json(&payload);

        if !api_key.trim().is_empty() {
            request_builder = request_builder.bearer_auth(api_key);
        }

        let response = request_builder.send().await.map_err(|error| {
            model_provider::redact_model_error_text(&format!("无法发送模型请求：{error}"))
        })?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_else(|_| String::new());
            if status.as_u16() == 400
                && payload.get("stream_options").is_some()
                && body.to_ascii_lowercase().contains("stream_options")
            {
                if let Some(object) = payload.as_object_mut() {
                    object.remove("stream_options");
                }
                log::info!(
                    target: "agent_runtime",
                    "provider 不接受 stream_options，已去掉后重试"
                );
                continue;
            }
            return Err(model_provider::redact_model_error_text(&format!(
                "模型请求失败：HTTP {status} {body}"
            )));
        }

        return read_chat_completion_response(response, on_progress).await;
    }
}

/** 把流式增量写进过程区：思考进 trace，回答进 live content。 */
fn apply_streamed_assistant_progress(
    tracer: &mut AgentTurnTracer,
    app: Option<&AppHandle>,
    streamed: &StreamedAssistant,
) {
    let progress = stream_ui_progress(streamed);
    if !progress.thinking.is_empty() {
        tracer.update_thinking(&progress.thinking, app);
    }
    tracer.set_partial_content(&progress.content, app);
}

/** 用模型增量合并会话工作记忆；失败时降级为确定性摘要，且不影响主 Agent 回合。 */
async fn update_agent_context_summary_best_effort(
    client: &Client,
    provider: &LlmProviderConfig,
    selected_model_id: &str,
    api_key: &str,
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    failure_reason: Option<&str>,
    update_compacted_marker: bool,
) {
    let started_at = Instant::now();
    let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
    let session_id = snapshot.sessions[session_index].id.clone();
    let messages =
        build_context_summary_model_messages(&snapshot.sessions[session_index], failure_reason);
    let response_format = context_summary_response_format();
    let result = send_chat_completion_logged(
        client,
        provider,
        selected_model_id,
        &endpoint,
        api_key,
        &messages,
        None,
        Some(&response_format),
    )
    .await
    .and_then(parse_context_summary_response);

    match result {
        Ok(summary) => {
            let summary = normalize_context_summary(
                summary,
                &snapshot.sessions[session_index],
                update_compacted_marker,
            );
            let rendered_chars = render_context_summary_body(&summary)
                .map(|body| body.chars().count())
                .unwrap_or_default();
            let field_count = context_summary_field_count(&summary);
            let updated_at = summary.updated_at.clone();

            snapshot.sessions[session_index].context_summary = Some(summary);
            snapshot.sessions[session_index].updated_at = format_local_datetime();
            log::info!(
                target: "agent_runtime",
                "会话工作记忆更新成功：session={} duration_ms={} rendered_chars={} field_count={} updated_at={}",
                session_id,
                started_at.elapsed().as_millis(),
                rendered_chars,
                field_count,
                updated_at
            );
        }
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "会话工作记忆模型更新失败，已使用确定性摘要：session={} duration_ms={} failure_reason_chars={} error={}",
                session_id,
                started_at.elapsed().as_millis(),
                failure_reason.map(|reason| reason.chars().count()).unwrap_or_default(),
                model_provider::redact_model_error_text(&error)
            );
            update_agent_context_summary_deterministic(
                snapshot,
                session_index,
                failure_reason,
                update_compacted_marker,
            );
        }
    }
}

/** 根据 自动触发条件决定使用模型 compact 还是轻量确定性同步。 */
async fn update_agent_context_summary_after_turn(
    client: &Client,
    provider: &LlmProviderConfig,
    selected_model_id: &str,
    api_key: &str,
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    estimated_prompt_chars: usize,
    failure_reason: Option<&str>,
    history_pack: Option<&PackedHistoryStats>,
    model_context_length: Option<u64>,
    silent_overflow: bool,
) -> bool {
    let decision = context_summary_auto_decision(
        &snapshot.sessions[session_index],
        estimated_prompt_chars,
        history_pack,
        Some(selected_model_id),
        model_context_length,
        silent_overflow,
    );

    log::debug!(
        target: "agent_runtime",
        "会话工作记忆自动触发检查：session={} should_compact={} reasons={} message_count={} unsummarized_messages={} estimated_prompt_chars={}",
        snapshot.sessions[session_index].id,
        decision.should_compact,
        if decision.reasons.is_empty() { "none".to_owned() } else { decision.reasons.join(",") },
        snapshot.sessions[session_index].messages.len(),
        decision.unsummarized_message_count,
        decision.estimated_prompt_chars
    );

    if decision.should_compact {
        update_agent_context_summary_best_effort(
            client,
            provider,
            selected_model_id,
            api_key,
            snapshot,
            session_index,
            failure_reason,
            true,
        )
        .await;
    } else {
        update_agent_context_summary_deterministic(snapshot, session_index, failure_reason, false);
    }
    decision.should_compact
}

/** 构造 summary-only 模型请求，不携带工具 schema，不进入用户可见消息列表。 */
fn build_context_summary_model_messages(
    session: &AgentSession,
    failure_reason: Option<&str>,
) -> Vec<Value> {
    let old_summary = session
        .context_summary
        .as_ref()
        .map(|summary| serde_json::to_string(summary).unwrap_or_default())
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| "null".to_owned());
    let recent_messages = context_summary_recent_message_payload(session);
    let pending_change =
        current_pending_change_summary(session).unwrap_or_else(|| "无待确认变更".to_owned());
    let turn_failure = failure_reason
        .map(truncate_summary_item)
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or_else(|| "无".to_owned());

    vec![
        json!({
            "role": "system",
            "content": "你负责维护橘记 Agent 会话工作记忆。只输出一个 JSON 对象，字段必须是 version、updatedAt、currentGoal、userConstraints、decisions、completedWork、pendingTasks、touchedNotes、pendingChangeSummary、openQuestions、lastSummarizedMessageId、lastCompactedMessageId。不要输出 Markdown。不要保存 API key、完整正文、完整 diff、手机号、身份证号或密码。每个数组最多 12 条，每条尽量短。"
        }),
        json!({
            "role": "user",
            "content": format!(
                "旧工作记忆 JSON：\n{}\n\n最近未整理消息和工具摘要 JSON：\n{}\n\n当前 pending diff 摘要：\n{}\n\n本轮失败摘要：\n{}\n\n请合并为新的工作记忆 JSON。",
                old_summary,
                recent_messages,
                pending_change,
                turn_failure
            )
        }),
    ]
}

/** 把最近消息压缩成 summary 模型可读 JSON，正文按预算截断且工具只保留摘要。 */
fn context_summary_recent_message_payload(session: &AgentSession) -> String {
    let last_compacted_id = session
        .context_summary
        .as_ref()
        .and_then(|summary| summary.last_compacted_message_id.as_deref());
    let mut messages = session
        .messages
        .iter()
        .skip_while(|message| Some(message.id.as_str()) != last_compacted_id)
        .skip(if last_compacted_id.is_some() { 1 } else { 0 })
        .collect::<Vec<_>>();

    if messages.is_empty() {
        messages = session
            .messages
            .iter()
            .rev()
            .take(MAX_RECENT_MESSAGES_FOR_SUMMARY)
            .collect::<Vec<_>>();
        messages.reverse();
    }

    let payload = messages
        .into_iter()
        .rev()
        .take(MAX_RECENT_MESSAGES_FOR_SUMMARY)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| {
            json!({
                "id": &message.id,
                "role": &message.role,
                "action": message.action.as_deref(),
                "content": truncate_chars(&message.content, MAX_HISTORY_MESSAGE_CHARS),
                "tools": message.tool_calls.as_ref().map(|tool_calls| {
                    tool_calls.iter().map(|tool_call| {
                        json!({
                            "name": &tool_call.name,
                            "status": &tool_call.status,
                            "summary": &tool_call.summary,
                        })
                    }).collect::<Vec<_>>()
                }).unwrap_or_default(),
                "citations": message.citations.as_ref().map(|citations| {
                    citations.iter().map(|citation| {
                        json!({
                            "noteId": &citation.note_id,
                            "title": &citation.title,
                        })
                    }).collect::<Vec<_>>()
                }).unwrap_or_default()
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_owned())
}

/** 解析 summary 模型响应；只接受 JSON object，兼容模型误包的 fenced code block。 */
fn parse_context_summary_response(response: Value) -> Result<AgentContextSummary, String> {
    let content = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .ok_or_else(|| "summary 响应缺少 content".to_owned())?;
    let json_text = extract_json_object_text(content)
        .ok_or_else(|| "summary 响应不是 JSON object".to_owned())?;
    let mut parsed: Value = serde_json::from_str(&json_text)
        .map_err(|error| format!("summary JSON 解析失败：{error}"))?;
    coerce_context_summary_json(&mut parsed);

    serde_json::from_value(parsed).map_err(|error| format!("summary JSON 解析失败：{error}"))
}

/** 兼容部分 provider 把 json_schema integer 写成字符串，避免整份工作记忆被丢掉。 */
fn coerce_context_summary_json(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(text) = object.get("version").and_then(Value::as_str) {
        if let Ok(number) = text.trim().parse::<u32>() {
            object.insert("version".to_owned(), json!(number));
        }
    }
}

/** 从模型响应中提取第一个 JSON object，避免 fenced JSON 导致解析失败。 */
fn extract_json_object_text(content: &str) -> Option<String> {
    let trimmed = content.trim();

    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed.to_owned());
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;

    (start < end).then(|| trimmed[start..=end].to_owned())
}

/** 模型总结失败或本地 fallback 时，用确定性规则维护最低可用工作记忆。 */
pub(crate) fn update_agent_context_summary_deterministic(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    failure_reason: Option<&str>,
    update_compacted_marker: bool,
) {
    let session = &snapshot.sessions[session_index];
    let mut summary = session
        .context_summary
        .clone()
        .unwrap_or_else(|| AgentContextSummary {
            version: 1,
            updated_at: format_local_datetime(),
            ..AgentContextSummary::default()
        });

    summary.version = 1;
    summary.updated_at = format_local_datetime();
    summary.current_goal = latest_user_message(session)
        .map(|message| truncate_summary_item(&message.content))
        .or(summary.current_goal);

    if let Some(last_assistant) = latest_assistant_message(session) {
        append_bounded_unique(
            &mut summary.completed_work,
            truncate_summary_item(&format!("本轮回复：{}", last_assistant.content)),
        );

        if let Some(tool_calls) = &last_assistant.tool_calls {
            for tool_call in tool_calls
                .iter()
                .filter(|tool_call| tool_call.status == "completed")
            {
                append_bounded_unique(
                    &mut summary.completed_work,
                    truncate_summary_item(&format!(
                        "工具 {}：{}",
                        tool_call.name, tool_call.summary
                    )),
                );
            }
        }
    }

    if let Some(reason) = failure_reason.filter(|reason| !reason.trim().is_empty()) {
        append_bounded_unique(
            &mut summary.pending_tasks,
            truncate_summary_item(&format!("上轮未完全成功，需要继续确认或重试：{reason}")),
        );
    }

    summary.pending_change_summary = current_pending_change_summary(session);
    if let Some(change_summary) = summary.pending_change_summary.clone() {
        if change_summary.contains("状态：pending") {
            append_bounded_unique(
                &mut summary.pending_tasks,
                "等待用户确认当前 pending diff。".to_owned(),
            );
        }
    }

    if let Some(change) = session
        .pending_change
        .as_ref()
        .filter(|change| change.status == "accepted" || change.status == "rejected")
    {
        append_bounded_unique(
            &mut summary.completed_work,
            truncate_summary_item(&format!(
                "待确认 diff 已处理：status={} title={} path={}",
                change.status, change.title, change.target_path
            )),
        );
    }

    merge_recent_citation_notes(&mut summary, session);
    summary.last_summarized_message_id = session.messages.last().map(|message| message.id.clone());
    summary = normalize_context_summary(summary, session, update_compacted_marker);

    let rendered_chars = render_context_summary_body(&summary)
        .map(|body| body.chars().count())
        .unwrap_or_default();
    let field_count = context_summary_field_count(&summary);

    snapshot.sessions[session_index].context_summary = Some(summary);
    snapshot.sessions[session_index].updated_at = format_local_datetime();
    let updated_at = snapshot.sessions[session_index]
        .context_summary
        .as_ref()
        .map(|summary| summary.updated_at.as_str())
        .unwrap_or("none");
    log::info!(
        target: "agent_runtime",
        "会话工作记忆确定性更新成功：session={} rendered_chars={} field_count={} updated_at={}",
        snapshot.sessions[session_index].id,
        rendered_chars,
        field_count,
        updated_at
    );
}

/** 规范模型或规则生成的工作记忆，统一长度、数量和 pending diff 状态。 */
fn normalize_context_summary(
    mut summary: AgentContextSummary,
    session: &AgentSession,
    update_compacted_marker: bool,
) -> AgentContextSummary {
    summary.version = if summary.version == 0 {
        1
    } else {
        summary.version
    };
    if summary.updated_at.trim().is_empty() {
        summary.updated_at = format_local_datetime();
    }
    summary.current_goal = summary
        .current_goal
        .filter(|goal| !goal.trim().is_empty())
        .map(|goal| truncate_summary_item(&goal));
    summary.user_constraints = normalize_summary_items(summary.user_constraints);
    summary.decisions = normalize_summary_items(summary.decisions);
    summary.completed_work = normalize_summary_items(summary.completed_work);
    summary.pending_tasks = normalize_summary_items(summary.pending_tasks);
    summary.open_questions = normalize_summary_items(summary.open_questions);
    summary.touched_notes = normalize_touched_notes(summary.touched_notes);
    summary.pending_change_summary = current_pending_change_summary(session);
    summary.last_summarized_message_id = session.messages.last().map(|message| message.id.clone());
    if update_compacted_marker {
        summary.last_compacted_message_id =
            session.messages.last().map(|message| message.id.clone());
    }

    summary
}

/** 返回 summary 中有内容的字段数量，供日志观测，不记录字段正文。 */
fn context_summary_field_count(summary: &AgentContextSummary) -> usize {
    usize::from(summary.current_goal.is_some())
        + usize::from(!summary.user_constraints.is_empty())
        + usize::from(!summary.decisions.is_empty())
        + usize::from(!summary.completed_work.is_empty())
        + usize::from(!summary.pending_tasks.is_empty())
        + usize::from(!summary.touched_notes.is_empty())
        + usize::from(summary.pending_change_summary.is_some())
        + usize::from(!summary.open_questions.is_empty())
        + usize::from(summary.last_summarized_message_id.is_some())
        + usize::from(summary.last_compacted_message_id.is_some())
}

/** 规范字符串数组字段，去空、去重、截断并限制条数。 */
fn normalize_summary_items(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for item in items {
        let item = truncate_summary_item(&item);

        if !item.is_empty() && seen.insert(item.clone()) {
            normalized.push(item);
        }

        if normalized.len() >= MAX_CONTEXT_SUMMARY_ITEMS {
            break;
        }
    }

    normalized
}

/** 规范 touched notes 字段，避免同一笔记重复占用 summary 预算。 */
fn normalize_touched_notes(notes: Vec<AgentContextTouchedNote>) -> Vec<AgentContextTouchedNote> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for note in notes {
        if note.id.trim().is_empty() || !seen.insert(note.id.clone()) {
            continue;
        }

        normalized.push(AgentContextTouchedNote {
            id: truncate_summary_item(&note.id),
            title: truncate_summary_item(&note.title),
            reason: truncate_summary_item(&note.reason),
        });

        if normalized.len() >= MAX_CONTEXT_SUMMARY_ITEMS {
            break;
        }
    }

    normalized
}

/** 截断单个 summary 字段，并折叠空白，避免长正文进入工作记忆。 */
fn truncate_summary_item(value: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");

    truncate_chars(&collapsed, MAX_CONTEXT_SUMMARY_ITEM_CHARS)
}

/** 向受限数组追加去重条目，超过预算时移除最旧条目。 */
fn append_bounded_unique(items: &mut Vec<String>, item: String) {
    if item.trim().is_empty() || items.iter().any(|existing| existing == &item) {
        return;
    }

    items.push(item);
    if items.len() > MAX_CONTEXT_SUMMARY_ITEMS {
        let overflow = items.len() - MAX_CONTEXT_SUMMARY_ITEMS;
        items.drain(0..overflow);
    }
}

/** 把最近消息引用的笔记合并进 touchedNotes，便于后续回合记住读过哪些笔记。 */
fn merge_recent_citation_notes(summary: &mut AgentContextSummary, session: &AgentSession) {
    let mut notes = summary.touched_notes.clone();

    for message in session
        .messages
        .iter()
        .rev()
        .take(MAX_RECENT_MESSAGES_FOR_SUMMARY)
    {
        if let Some(citations) = &message.citations {
            for citation in citations {
                notes.push(AgentContextTouchedNote {
                    id: citation.note_id.clone(),
                    title: citation.title.clone(),
                    reason: "本会话工具读取或引用过。".to_owned(),
                });
            }
        }
    }

    summary.touched_notes = normalize_touched_notes(notes);
}

/** 查找最新用户消息，供确定性 summary 更新当前目标。 */
fn latest_user_message(session: &AgentSession) -> Option<&AgentMessage> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
}

/** 查找最新 assistant 消息，供确定性 summary 记录本轮完成项。 */
fn latest_assistant_message(session: &AgentSession) -> Option<&AgentMessage> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
}

/** 在模型未配置或失败时运行本地规则 Agent，并生成对应审计。 */
fn fallback_agent_turn(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    available_skills: &[AgentSkill],
    reason: &str,
) -> RuntimeTurnResult {
    let mut tracer = AgentTurnTracer::new(request.session_id.clone(), create_id("assistant"));
    tracer.emit_started(Some(app));
    let mut turn_result = agent::run_agent_turn(app, snapshot, request.clone());
    let session_index = turn_result
        .snapshot
        .sessions
        .iter()
        .position(|session| session.id == request.session_id)
        .unwrap_or(0);

    if let Some(last_message) = turn_result.snapshot.sessions[session_index]
        .messages
        .last_mut()
        .filter(|message| message.role == "assistant")
    {
        // 把降级原因插入本轮工具轨迹开头，避免 UI 看起来像真实模型回答。
        last_message
            .tool_calls
            .get_or_insert_with(Vec::new)
            .insert(0, local_rule_tool_call(reason));
        last_message
            .tool_calls
            .get_or_insert_with(Vec::new)
            .insert(0, skill_context_tool_call(available_skills));
        last_message.id = tracer.live_message_id().to_owned();
        tracer.ingest_completed_tools(last_message.tool_calls.as_deref().unwrap_or(&[]));
        last_message.trace = tracer.steps();
        tracer.finish(Some(&last_message.content), Some(app));
        last_message.turn_duration_ms = Some(tracer.duration_ms());
    } else {
        tracer.finish(None, Some(app));
    }

    update_agent_context_summary_deterministic(
        &mut turn_result.snapshot,
        session_index,
        Some(reason),
        false,
    );

    let audit_log = build_audit_log(
        "local_rule_turn",
        &turn_result.snapshot,
        session_index,
        &request.prompt,
        &format!("{reason}；{}", skills::skill_summary(available_skills)),
        &RuntimeAuditTrail::default(),
    );

    RuntimeTurnResult {
        turn_result,
        audit_log,
    }
}

/** 确保本轮用户消息存在；前端已乐观落库时复用同一条消息，避免最终快照重复。 */
fn ensure_user_message_for_turn(session: &mut AgentSession, request: &AgentTurnRequest) -> String {
    let user_message_id = request
        .client_message_id
        .clone()
        .unwrap_or_else(|| create_id("user"));

    if session
        .messages
        .iter_mut()
        .find(|message| message.id == user_message_id && message.role == "user")
        .map(|message| {
            // 前端乐观消息可能尚未带该字段；后端以本轮已提交请求为准补齐历史回显数据。
            message.mentioned_file_ids = request.mentioned_file_ids.clone();
        })
        .is_some()
    {
        return user_message_id;
    }

    session
        .messages
        .push(build_user_message(request, user_message_id.clone()));
    user_message_id
}

/** 构造用户消息，确保真实模型、错误分支和本地 fallback 的消息形态一致。 */
fn build_user_message(request: &AgentTurnRequest, id: String) -> AgentMessage {
    AgentMessage {
        id,
        role: "user".to_owned(),
        content: request.prompt.clone(),
        action: Some(request.action.clone()),
        citations: None,
        tool_calls: None,
        mentioned_file_ids: request.mentioned_file_ids.clone(),
        trace: Vec::new(),
        turn_duration_ms: None,
    }
}

/** 构造模型请求轨迹；args 只记录非敏感配置，绝不包含 API key。 */
fn model_request_tool_call(
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    status: &str,
) -> AgentToolCall {
    AgentToolCall {
        id: create_id("tool"),
        name: "model_request".to_owned(),
        status: status.to_owned(),
        summary: format!(
            "{}（{}）模型请求：{} @ {}",
            provider.name, provider.provider, model_id, endpoint
        ),
        args: json!({
            "providerId": provider.id,
            "providerName": provider.name,
            "provider": provider.provider,
            "apiBase": provider.api_base,
            "model": model_id,
            "endpoint": endpoint
        }),
    }
}

/** 构造本地规则 Agent 轨迹，让 UI 明确显示本轮没有调用云端模型。 */
fn local_rule_tool_call(reason: &str) -> AgentToolCall {
    AgentToolCall {
        id: create_id("tool"),
        name: "local_rule_agent".to_owned(),
        status: "completed".to_owned(),
        summary: reason.to_owned(),
        args: json!({ "reason": reason }),
    }
}

/** 构造本轮 skill 上下文轨迹，记录已注入给模型参考的启用 Skill 目录。 */
fn skill_context_tool_call(available_skills: &[AgentSkill]) -> AgentToolCall {
    let enabled_skills = available_skills
        .iter()
        .filter(|skill| skill.enabled)
        .collect::<Vec<_>>();

    AgentToolCall {
        id: create_id("tool"),
        name: "skill_context".to_owned(),
        status: "completed".to_owned(),
        summary: skills::skill_summary(available_skills),
        args: json!({
            "enabledSkillCount": enabled_skills.len(),
            "skills": enabled_skills
                .into_iter()
                .map(|skill| {
                    json!({
                        "skillId": skill.id,
                        "name": skill.name,
                        "displayName": skill.display_name,
                        "source": skill.source,
                        "path": skill.path,
                        "relativePath": skill.relative_path,
                    })
                })
                .collect::<Vec<_>>()
        }),
    }
}

/** 构造显式 Skill 激活轨迹；args 只含元数据和字符数，不暴露 instructions 正文。 */
fn activate_skill_tool_calls(
    explicit_skills: &[AgentSkill],
    status: &str,
    failed_reason: Option<&str>,
) -> Vec<AgentToolCall> {
    explicit_skills
        .iter()
        .map(|skill| {
            let instruction_chars = skill.instructions.chars().count();
            let summary = match (status, failed_reason) {
                ("failed", Some(reason)) => {
                    format!("显式 Skill「{}」未完成执行：{}", skill.display_name, reason)
                }
                ("failed", None) => format!("显式 Skill「{}」未完成执行。", skill.display_name),
                _ => format!(
                    "已显式激活 Skill「{}」，instructions {} 字符已进入本轮模型上下文。",
                    skill.display_name, instruction_chars
                ),
            };

            AgentToolCall {
                id: create_id("tool"),
                name: "activate_skill".to_owned(),
                status: status.to_owned(),
                summary,
                args: json!({
                    "skillId": skill.id,
                    "name": skill.name,
                    "displayName": skill.display_name,
                    "source": skill.source,
                    "relativePath": skill.relative_path,
                    "instructionChars": instruction_chars,
                }),
            }
        })
        .collect()
}

/** 构造无法解析到具体 Skill 时的失败激活轨迹，避免丢失显式选择失败原因。 */
fn failed_activate_skill_request_tool_call(
    requested_skill_ids: &[String],
    reason: &str,
) -> AgentToolCall {
    let sanitized_ids = requested_skill_ids
        .iter()
        .map(|skill_id| skill_id.trim())
        .filter(|skill_id| !skill_id.is_empty())
        .take(MAX_EXPLICIT_SKILLS_PER_TURN)
        .collect::<Vec<_>>();

    AgentToolCall {
        id: create_id("tool"),
        name: "activate_skill".to_owned(),
        status: "failed".to_owned(),
        summary: format!("显式 Skill 激活失败：{reason}"),
        args: json!({
            "skillIds": sanitized_ids,
            "requestedSkillCount": requested_skill_ids.len(),
            "instructionChars": 0,
            "reason": reason,
        }),
    }
}

/** 云端模型启用后发生配置或请求错误时，返回可见错误消息而不是静默降级；reason 会先脱敏再展示。 */
fn model_error_turn(
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    provider: Option<&LlmProviderConfig>,
    selected_model_id: Option<&str>,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    reason: &str,
    tracer: Option<&AgentTurnTracer>,
) -> RuntimeTurnResult {
    let session_index = resolve_session_index(&snapshot, &request).unwrap_or(0);
    let redacted_reason = model_provider::redact_model_error_text(reason);
    let failed_request = match provider {
        Some(provider) => {
            let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
            // 失败发生在选型之后时，工具轨迹必须记录本轮最终模型 ID，不能退回 provider 默认值。
            let model_id = selected_model_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(provider.model.as_str());
            let mut call = model_request_tool_call(provider, model_id, &endpoint, "failed");

            call.summary = redacted_reason.clone();
            call
        }
        None => AgentToolCall {
            id: create_id("tool"),
            name: "model_request".to_owned(),
            status: "failed".to_owned(),
            summary: redacted_reason.clone(),
            args: json!({ "reason": redacted_reason }),
        },
    };

    apply_first_prompt_title(&mut snapshot.sessions[session_index], &request.prompt);
    ensure_user_message_for_turn(&mut snapshot.sessions[session_index], &request);
    let mut tool_calls = vec![skill_context_tool_call(available_skills)];

    tool_calls.extend(activate_skill_tool_calls(
        explicit_skills,
        if explicit_skills.is_empty() {
            "completed"
        } else {
            "failed"
        },
        Some("真实模型请求没有完成，strict skill execution 未发生。"),
    ));
    tool_calls.push(failed_request);
    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: tracer
                .map(|tracer| tracer.live_message_id().to_owned())
                .unwrap_or_else(|| create_id("assistant")),
            role: "assistant".to_owned(),
            content: if explicit_skills.is_empty() {
                format!("真实模型请求没有完成：{redacted_reason}")
            } else {
                format!("显式 Skill 未完成执行：{redacted_reason}")
            },
            action: Some(request.action.clone()),
            citations: Some(Vec::new()),
            tool_calls: Some(tool_calls),
            mentioned_file_ids: Vec::new(),
            trace: tracer
                .map(|tracer| tracer.steps())
                .filter(|steps| !steps.is_empty())
                .unwrap_or_default(),
            turn_duration_ms: tracer.map(|tracer| tracer.duration_ms()),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();
    update_agent_context_summary_deterministic(
        &mut snapshot,
        session_index,
        Some(&redacted_reason),
        false,
    );

    let audit_log = build_audit_log(
        "model_error_turn",
        &snapshot,
        session_index,
        &request.prompt,
        &format!(
            "{redacted_reason}；{}",
            format_skill_audit_summary(available_skills, explicit_skills)
        ),
        &RuntimeAuditTrail::default(),
    );

    RuntimeTurnResult {
        turn_result: AgentTurnResult { snapshot },
        audit_log,
    }
}

/** 显式 Skill 无法进入真实模型 turn 时返回可见错误，不能静默降级成本地规则 Agent。 */
fn skill_activation_error_turn(
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    reason: &str,
) -> RuntimeTurnResult {
    let session_index = resolve_session_index(&snapshot, &request).unwrap_or(0);
    let redacted_reason = model_provider::redact_model_error_text(reason);
    let mut tool_calls = vec![skill_context_tool_call(available_skills)];

    if explicit_skills.is_empty() {
        tool_calls.push(failed_activate_skill_request_tool_call(
            &request.explicit_skill_ids,
            &redacted_reason,
        ));
    } else {
        tool_calls.extend(activate_skill_tool_calls(
            explicit_skills,
            "failed",
            Some("当前配置无法执行真实模型 turn，strict skill execution 未发生。"),
        ));
        tool_calls.push(AgentToolCall {
            id: create_id("tool"),
            name: "model_request".to_owned(),
            status: "failed".to_owned(),
            summary: redacted_reason.clone(),
            args: json!({ "reason": redacted_reason }),
        });
    }

    apply_first_prompt_title(&mut snapshot.sessions[session_index], &request.prompt);
    ensure_user_message_for_turn(&mut snapshot.sessions[session_index], &request);
    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content: format!("显式 Skill 未完成执行：{redacted_reason}"),
            action: Some(request.action.clone()),
            citations: Some(Vec::new()),
            tool_calls: Some(tool_calls),
            mentioned_file_ids: Vec::new(),
            trace: Vec::new(),
            turn_duration_ms: None,
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();
    update_agent_context_summary_deterministic(
        &mut snapshot,
        session_index,
        Some(&redacted_reason),
        false,
    );

    log::warn!(
        target: "agent_runtime",
        "显式 Skill 执行中止：session={} requested_count={} resolved_count={} reason={}",
        snapshot.sessions[session_index].id,
        request.explicit_skill_ids.len(),
        explicit_skills.len(),
        redacted_reason
    );

    let explicit_summary = if explicit_skills.is_empty() {
        format!(
            "显式 Skill：{} 个（解析失败或未完成校验）",
            request
                .explicit_skill_ids
                .iter()
                .filter(|skill_id| !skill_id.trim().is_empty())
                .count()
        )
    } else {
        skills::explicit_skill_summary(explicit_skills)
    };
    let audit_log = build_audit_log(
        "skill_activation_error_turn",
        &snapshot,
        session_index,
        &request.prompt,
        &format!(
            "{redacted_reason}；{}；{}",
            skills::skill_summary(available_skills),
            explicit_summary
        ),
        &RuntimeAuditTrail::default(),
    );

    RuntimeTurnResult {
        turn_result: AgentTurnResult { snapshot },
        audit_log,
    }
}

/** 根据 sessionId 查找会话索引。 */
fn resolve_session_index(
    snapshot: &WorkspaceSnapshot,
    request: &AgentTurnRequest,
) -> Result<usize, String> {
    snapshot
        .sessions
        .iter()
        .position(|session| session.id == request.session_id)
        .or_else(|| {
            snapshot
                .sessions
                .iter()
                .position(|session| session.id == snapshot.active_session_id)
        })
        .or_else(|| (!snapshot.sessions.is_empty()).then_some(0))
        .ok_or_else(|| "当前没有可用 Agent 会话。".to_owned())
}

/** 空白新会话的标题直接使用用户第一条输入，避免按知识库或文档名组装默认标题。 */
fn apply_first_prompt_title(session: &mut AgentSession, prompt: &str) {
    let has_user_message = session
        .messages
        .iter()
        .any(|message| message.role == "user");

    if !has_user_message && session.title.trim() == "新会话" {
        let next_title = prompt.trim();

        if !next_title.is_empty() {
            session.title = next_title.to_owned();
        }
    }
}

/** 追加 assistant 消息并更新时间。 */
fn push_assistant_message(
    snapshot: &mut WorkspaceSnapshot,
    session_index: usize,
    action: &str,
    content: String,
    citations: Vec<Citation>,
    tool_calls: Vec<AgentToolCall>,
    tracer: &AgentTurnTracer,
) {
    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: tracer.live_message_id().to_owned(),
            role: "assistant".to_owned(),
            content,
            action: Some(action.to_owned()),
            citations: Some(deduplicate_citations(citations)),
            tool_calls: Some(tool_calls),
            mentioned_file_ids: Vec::new(),
            trace: tracer.steps(),
            turn_duration_ms: Some(tracer.duration_ms()),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();
}

/** 去重引用，避免 search 和 read 返回同一笔记时重复展示。 */
fn deduplicate_citations(citations: Vec<Citation>) -> Vec<Citation> {
    let mut seen_note_ids = HashSet::new();
    let mut next_citations = Vec::new();

    for citation in citations {
        if seen_note_ids.insert(citation.note_id.clone()) {
            next_citations.push(citation);
        }
    }

    next_citations
}

/** 构造审计日志，记录模型请求或本地规则 fallback 的 scope 与工具摘要。 */
fn build_audit_log(
    kind: &str,
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    prompt: &str,
    content_summary: &str,
    audit_trail: &RuntimeAuditTrail,
) -> RequestAuditLog {
    let session = &snapshot.sessions[session_index];
    let scope_summary = build_scope_summary(snapshot, session);
    let tool_summary = session
        .messages
        .last()
        .and_then(|message| message.tool_calls.as_ref())
        .map(|tool_calls| {
            tool_calls
                .iter()
                .map(|tool_call| tool_call.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| "未调用工具".to_owned());

    RequestAuditLog {
        id: create_id("audit"),
        kind: kind.to_owned(),
        session_id: Some(session.id.clone()),
        scope_summary,
        content_summary: audit_trail.content_summary(content_summary, prompt, session),
        tool_summary,
        created_at: format_local_datetime(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AgentTraceStep, FolderEntry, KnowledgeBase, Note};
    use crate::storage::hash_content;
    use std::fs;
    use tempfile::tempdir;

    /** 构造 Runtime 单元测试使用的最小工作台快照。 */
    fn runtime_test_snapshot(note_content: String) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            knowledge_bases: vec![
                KnowledgeBase {
                    id: "kb-a".to_owned(),
                    name: "主知识库".to_owned(),
                    path: "/tmp/kb-a".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: true,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
                KnowledgeBase {
                    id: "kb-b".to_owned(),
                    name: "未授权知识库".to_owned(),
                    path: "/tmp/kb-b".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: false,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
            ],
            folders: vec![FolderEntry {
                id: "folder-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                name: "Notes".to_owned(),
                path: "Notes".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            notes: vec![
                Note {
                    id: "note-a".to_owned(),
                    knowledge_base_id: "kb-a".to_owned(),
                    title: "授权笔记".to_owned(),
                    path: "Notes/授权笔记.md".to_owned(),
                    content_hash: hash_content(&note_content),
                    content: note_content,
                    tags: vec!["测试".to_owned()],
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
                Note {
                    id: "note-b".to_owned(),
                    knowledge_base_id: "kb-b".to_owned(),
                    title: "未授权笔记".to_owned(),
                    path: "Private/未授权笔记.md".to_owned(),
                    content_hash: hash_content("private"),
                    content: "private".to_owned(),
                    tags: Vec::new(),
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
            ],
            documents: Vec::new(),
            sessions: vec![AgentSession {
                id: "session-a".to_owned(),
                title: "测试会话".to_owned(),
                im_identity: None,
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: vec!["note-a".to_owned()],
                messages: Vec::new(),
                pending_change: None,
                pending_change_set: None,
                pending_execution: None,
                security_level: "basic".to_owned(),
                context_summary: None,
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
                deleted_at: None,
                model_provider_id: None,
                model_id: None,
                context_usage: None,
            }],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        }
    }

    /** 构造历史回放测试用的会话消息，默认不含工具轨迹。 */
    fn history_test_message(
        id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<Vec<AgentToolCall>>,
        trace: Vec<AgentTraceStep>,
    ) -> AgentMessage {
        AgentMessage {
            id: id.to_owned(),
            role: role.to_owned(),
            content: content.to_owned(),
            action: Some("ask".to_owned()),
            citations: None,
            tool_calls,
            mentioned_file_ids: Vec::new(),
            trace,
            turn_duration_ms: None,
        }
    }

    /** 构造 Runtime 单元测试使用的 Agent 请求。 */
    fn runtime_test_request(action: &str, prompt: &str) -> AgentTurnRequest {
        AgentTurnRequest {
            prompt: prompt.to_owned(),
            action: action.to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
            mentioned_file_ids: Vec::new(),
        }
    }

    /** 多知识库 scope 摘要必须带上 id，避免模型和 list_tree 的 knowledgeBaseId 对不上号。 */
    #[test]
    fn build_scope_summary_includes_knowledge_base_ids() {
        let mut snapshot = runtime_test_snapshot("授权笔记".to_owned());
        snapshot.knowledge_bases[0].name = "jd调研".to_owned();
        snapshot.knowledge_bases[1].name = "橘记".to_owned();
        snapshot.sessions[0].knowledge_base_ids = vec!["kb-a".to_owned(), "kb-b".to_owned()];

        let summary = build_scope_summary(&snapshot, &snapshot.sessions[0]);

        assert_eq!(summary, "2 个知识库：jd调研 (id=kb-a) / 橘记 (id=kb-b)");
    }

    /** @ 文件必须重新受会话 scope 约束，重复项去重且文本正文仅注入允许的 Markdown/TXT。 */
    #[test]
    fn mentioned_files_filter_scope_duplicates_and_inject_text() {
        let mut snapshot = runtime_test_snapshot("授权 Markdown 正文".to_owned());
        snapshot.documents.push(crate::domain::WorkspaceDocument {
            id: "text-a".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "授权文本".to_owned(),
            path: "Materials/a.txt".to_owned(),
            file_type: "txt".to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: "hash".to_owned(),
            content: Some("TXT 显式材料正文".to_owned()),
            preview_available: true,
        });
        let mut request = runtime_test_request("ask", "参考材料");
        request.mentioned_file_ids = vec![
            "note-a".to_owned(),
            "note-a".to_owned(),
            "note-b".to_owned(),
            "text-a".to_owned(),
            "missing".to_owned(),
        ];
        let materials = resolve_mentioned_files(&snapshot, &snapshot.sessions[0], &request);
        let prompt = render_mentioned_files_prompt(&materials).unwrap();

        assert_eq!(materials.len(), 2);
        assert!(prompt.contains("授权 Markdown 正文"));
        assert!(prompt.contains("TXT 显式材料正文"));
        assert!(!prompt.contains("private"));
    }

    /** 同知识库图片仅生成相对当前 Markdown 的引用，跨库或非 Markdown 当前文件不生成。 */
    #[test]
    fn mentioned_image_exposes_safe_relative_markdown_path() {
        let mut snapshot = runtime_test_snapshot("正文".to_owned());
        snapshot.notes[0].path = "Notes/目标.md".to_owned();
        snapshot.documents.push(crate::domain::WorkspaceDocument {
            id: "image-a".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "图示".to_owned(),
            path: "assets/diagram.png".to_owned(),
            file_type: "image".to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: "hash".to_owned(),
            content: None,
            preview_available: true,
        });
        let mut request = runtime_test_request("ask", "插入图片");
        request.mentioned_file_ids = vec!["image-a".to_owned()];
        let materials = resolve_mentioned_files(&snapshot, &snapshot.sessions[0], &request);

        assert_eq!(
            materials[0].image_markdown_path.as_deref(),
            Some("../assets/diagram.png")
        );
        assert!(render_mentioned_files_prompt(&materials)
            .unwrap()
            .contains("![](../assets/diagram.png)"));
    }

    /** 构造已启用云端模型的测试设置，默认 provider 指向测试 endpoint 和模型。 */
    fn runtime_test_settings() -> UserSettings {
        let mut settings = crate::storage::default_user_settings();

        settings.model_config.enabled = true;
        settings.model_config.providers[0].enabled = true;
        settings.model_config.providers[0].api_base = "https://llm.example/v1".to_owned();
        settings.model_config.providers[0].model = "test-model".to_owned();

        settings
    }

    /** 构造已启用云端模型测试设置中的默认 provider，供直接传给 runtime 内部函数使用。 */
    fn runtime_test_provider() -> LlmProviderConfig {
        runtime_test_settings().model_config.providers[0].clone()
    }

    /** 构造测试用待确认变更，正文只用于验证 prompt 和 summary 不泄露完整 diff。 */
    fn runtime_test_pending_change(status: &str) -> ProposedChange {
        ProposedChange {
            id: "change-a".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            note_id: Some("note-a".to_owned()),
            target_id: Some("note-a".to_owned()),
            target_kind: Some("note".to_owned()),
            file_type: Some("markdown".to_owned()),
            r#type: "rewrite".to_owned(),
            operation: Some("replace".to_owned()),
            title: "授权笔记".to_owned(),
            target_path: "Notes/授权笔记.md".to_owned(),
            original: "旧正文里有较长内容".to_owned(),
            next: "新正文里有较长内容".to_owned(),
            original_hash: hash_content("旧正文里有较长内容"),
            status: status.to_owned(),
            review_comments: None,
            review_state: None,
            diff_stats: Some(crate::domain::ProposedChangeDiffStats {
                added_lines: 2,
                removed_lines: 1,
                context_lines: 3,
                hunk_count: 1,
                original_line_count: 4,
                next_line_count: 5,
                original_char_count: 9,
                next_char_count: 9,
            }),
        }
    }

    /** System prompt 应把工具选择权交给模型，而不是由宿主分支决定。 */
    #[test]
    fn model_messages_delegate_tool_choice_to_model() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结当前知识库里的隐私边界");
        let available_skills = crate::skills::built_in_skills();
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let system_content = messages[0]["content"].as_str().unwrap_or_default();

        assert!(system_content.contains("自主判断是否调用工具"));
        assert!(!system_content.contains("本轮很可能需要"));
        assert!(system_content.contains("主知识库"));
        assert!(system_content.contains("<available_skills>"));
        assert!(system_content.contains("location=\"built-in\""));
        assert!(system_content.contains("name=\"note-research\""));
        assert!(system_content.contains("是否使用、使用哪一个 Skill 都由你自主判断"));
        assert!(!system_content.contains("执行要求"));
        assert!(!system_content.contains("当用户要求查找"));
        assert!(!system_content.contains("本轮显式激活的 Skills：无"));
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
    }

    /** 会话工作记忆必须在 system 指令之后、短期历史之前注入，确保被预算裁掉的早期目标仍能保留。 */
    #[test]
    fn model_messages_inject_context_summary_before_recent_history() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续处理");
        let available_skills = crate::skills::built_in_skills();

        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-07-08 10:00:00".to_owned(),
            current_goal: Some("按产品分析框架整理这篇文章".to_owned()),
            user_constraints: vec!["保留用户已确认的小标题".to_owned()],
            decisions: vec!["采用问题-洞察-行动的结构".to_owned()],
            completed_work: vec!["已读取授权笔记".to_owned()],
            pending_tasks: vec!["下一轮继续生成待确认 diff".to_owned()],
            touched_notes: vec![AgentContextTouchedNote {
                id: "note-a".to_owned(),
                title: "授权笔记".to_owned(),
                reason: "本会话已读取。".to_owned(),
            }],
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("user-old".to_owned()),
            last_compacted_message_id: Some("user-old".to_owned()),
        });
        snapshot.sessions[0].messages = vec![
            history_test_message(
                "user-old",
                "user",
                &"早期目标正文需要被检查点代替。".repeat(400),
                None,
                Vec::new(),
            ),
            AgentMessage {
                id: "user-current".to_owned(),
                role: "user".to_owned(),
                content: "继续处理".to_owned(),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: None,
                mentioned_file_ids: Vec::new(),
                trace: Vec::new(),
                turn_duration_ms: None,
            },
        ];

        let messages = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            Some(4_096),
            None,
        )
        .messages;
        let memory_content = messages[1]["content"].as_str().unwrap_or_default();
        let system_content = messages[0]["content"].as_str().unwrap_or_default();

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert!(memory_content.contains("压缩检查点"));
        assert!(memory_content.contains("按产品分析框架整理这篇文章"));
        assert!(!memory_content.contains("version:"));
        assert!(!memory_content.contains("updatedAt:"));
        assert!(!system_content.contains("压缩检查点"));
        assert!(!system_content.contains("【会话工作记忆】"));
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
    }

    /** 历史 assistant 必须按 chat completions 协议回放 tool_calls 和 tool 结果，不能只剩截断正文。 */
    #[test]
    fn model_messages_replay_assistant_tool_calls_and_tool_results() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续刚才的检索");
        let available_skills = crate::skills::built_in_skills();

        snapshot.sessions[0].messages = vec![
            history_test_message("user-old", "user", "帮我找隐私边界", None, Vec::new()),
            AgentMessage {
                id: "assistant-old".to_owned(),
                role: "assistant".to_owned(),
                content: "我先检索相关笔记。".to_owned(),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: Some(vec![
                    AgentToolCall {
                        id: "host-skill-context".to_owned(),
                        name: "skill_context".to_owned(),
                        status: "completed".to_owned(),
                        summary: "已加载 Skill 目录".to_owned(),
                        args: json!({ "count": 2 }),
                    },
                    AgentToolCall {
                        id: "call-search-notes".to_owned(),
                        name: "search_notes".to_owned(),
                        status: "completed".to_owned(),
                        summary: "已检索到 2 条笔记".to_owned(),
                        args: json!({ "query": "隐私边界" }),
                    },
                    AgentToolCall {
                        id: "host-model-request".to_owned(),
                        name: "model_request".to_owned(),
                        status: "completed".to_owned(),
                        summary: "模型请求完成".to_owned(),
                        args: json!({ "model": "gpt-4o-mini" }),
                    },
                ]),
                mentioned_file_ids: Vec::new(),
                trace: vec![AgentTraceStep {
                    id: "trace-search".to_owned(),
                    step_type: "tool".to_owned(),
                    timestamp: "刚刚".to_owned(),
                    content: None,
                    name: Some("search_notes".to_owned()),
                    status: Some("completed".to_owned()),
                    summary: Some("已检索到 2 条笔记".to_owned()),
                    args: Some(json!({ "query": "隐私边界" })),
                    result_preview: Some(
                        r#"{"matches":[{"title":"隐私边界","score":0.9}]}"#.to_owned(),
                    ),
                    error: None,
                    duration_ms: Some(12),
                }],
                turn_duration_ms: Some(1200),
            },
            history_test_message("user-current", "user", "继续刚才的检索", None, Vec::new()),
        ];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("history should include assistant tool call message");
        let tool_calls = assistant["tool_calls"]
            .as_array()
            .expect("assistant history should include tool_calls");
        let tool = messages
            .iter()
            .find(|message| message["role"] == "tool")
            .expect("history should include tool result message");

        assert_eq!(assistant["content"], "我先检索相关笔记。");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call-search-notes");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "search_notes");
        assert!(tool_calls[0]["function"]["arguments"]
            .as_str()
            .unwrap_or_default()
            .contains("隐私边界"));
        assert_eq!(tool["tool_call_id"], "call-search-notes");
        assert!(tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("隐私边界"));
        assert!(messages.iter().all(|message| {
            message["tool_calls"]
                .as_array()
                .map(|tool_calls| {
                    tool_calls.iter().all(|tool_call| {
                        !matches!(
                            tool_call["function"]["name"].as_str(),
                            Some("skill_context" | "model_request")
                        )
                    })
                })
                .unwrap_or(true)
        }));
    }

    /** 没有模型工具的 assistant 历史仍只发 role/content，避免凭空补 tool_calls。 */
    #[test]
    fn model_messages_without_model_tools_stay_content_only() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![
            history_test_message("user-old", "user", "你好", None, Vec::new()),
            AgentMessage {
                id: "assistant-old".to_owned(),
                role: "assistant".to_owned(),
                content: "这是普通回答。".to_owned(),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: Some(vec![AgentToolCall {
                    id: "host-skill-context".to_owned(),
                    name: "skill_context".to_owned(),
                    status: "completed".to_owned(),
                    summary: "已加载 Skill 目录".to_owned(),
                    args: json!({ "count": 1 }),
                }]),
                mentioned_file_ids: Vec::new(),
                trace: Vec::new(),
                turn_duration_ms: None,
            },
            history_test_message("user-current", "user", "继续", None, Vec::new()),
        ];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("history should include assistant message");

        assert_eq!(assistant["content"], "这是普通回答。");
        assert!(assistant.get("tool_calls").is_none());
        assert!(messages.iter().all(|message| message["role"] != "tool"));
    }

    /** 超长工具参数必须仍是合法 JSON，不能把 arguments 字符串从中间截断。 */
    #[test]
    fn model_messages_tool_arguments_remain_valid_json_when_truncated() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("rewrite", "继续改写");
        let available_skills = crate::skills::built_in_skills();
        let long_original = "旧段落内容".repeat(400);
        snapshot.sessions[0].messages = vec![AgentMessage {
            id: "assistant-old".to_owned(),
            role: "assistant".to_owned(),
            content: "准备替换这段。".to_owned(),
            action: Some("rewrite".to_owned()),
            citations: None,
            tool_calls: Some(vec![AgentToolCall {
                id: "call-propose".to_owned(),
                name: "propose_file_change".to_owned(),
                status: "completed".to_owned(),
                summary: "已生成待确认 diff".to_owned(),
                args: json!({
                    "fileId": "note-a",
                    "operation": "replace",
                    "original": long_original,
                    "next": "新段落"
                }),
            }]),
            mentioned_file_ids: Vec::new(),
            trace: Vec::new(),
            turn_duration_ms: None,
        }];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("history should include assistant tool call message");
        let arguments = assistant["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("tool arguments should be a JSON string");
        let parsed: Value = serde_json::from_str(arguments)
            .expect("truncated tool arguments must remain valid JSON");

        assert_eq!(parsed["fileId"], "note-a");
        assert_eq!(parsed["operation"], "replace");
        assert!(
            arguments.chars().count() <= MAX_HISTORY_MESSAGE_CHARS,
            "tool arguments should stay within history budget, got {}",
            arguments.chars().count()
        );
    }

    /** 过程区没有 result_preview 时，tool 结果回退为 status + summary，保证协议完整。 */
    #[test]
    fn model_messages_tool_history_falls_back_to_status_and_summary() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续改写");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![AgentMessage {
            id: "assistant-old".to_owned(),
            role: "assistant".to_owned(),
            content: String::new(),
            action: Some("rewrite".to_owned()),
            citations: None,
            tool_calls: Some(vec![AgentToolCall {
                id: "call-read-file".to_owned(),
                name: "read_file".to_owned(),
                status: "failed".to_owned(),
                summary: "目标文件不在当前 scope 内".to_owned(),
                args: json!({ "fileId": "note-missing" }),
            }]),
            mentioned_file_ids: Vec::new(),
            trace: Vec::new(),
            turn_duration_ms: None,
        }];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .expect("history should include assistant tool call message");
        let tool = messages
            .iter()
            .find(|message| message["role"] == "tool")
            .expect("history should include tool result message");

        assert_eq!(assistant["content"], "");
        assert_eq!(tool["tool_call_id"], "call-read-file");
        assert!(tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("failed"));
        assert!(tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("目标文件不在当前 scope 内"));
    }

    /** 未知窗口走 256k 默认；已知中等窗口按比例装箱，更大窗口不超过 256k 对应的历史上限。 */
    #[test]
    fn history_budget_follows_model_context_length() {
        let unknown = resolve_history_budget_chars(None);
        let small = resolve_history_budget_chars(Some(4_096));
        let mid = resolve_history_budget_chars(Some(128_000));
        let large = resolve_history_budget_chars(Some(500_000));

        assert_eq!(unknown, 204_800);
        assert_eq!(small, MIN_KNOWN_HISTORY_BUDGET_CHARS);
        assert_eq!(mid, 102_400);
        assert_eq!(large, MAX_HISTORY_BUDGET_CHARS);
        assert_eq!(unknown, MAX_HISTORY_BUDGET_CHARS);
        assert!(small < mid);
        assert!(mid < unknown);
    }

    /** 短消息长会话应按窗口预算装入，而不是硬切最近 8 条。 */
    #[test]
    fn model_messages_keep_more_than_eight_short_history_turns() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续第 12 轮");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = (0..12)
            .flat_map(|index| {
                vec![
                    history_test_message(
                        &format!("user-{index}"),
                        "user",
                        &format!("用户轮次 {index} 的目标"),
                        None,
                        Vec::new(),
                    ),
                    history_test_message(
                        &format!("assistant-{index}"),
                        "assistant",
                        &format!("助手轮次 {index} 的回复"),
                        None,
                        Vec::new(),
                    ),
                ]
            })
            .collect();

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-11",
            &[],
            None,
            None,
        );
        let user_contents = prompt
            .messages
            .iter()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>();

        assert_eq!(prompt.history.included_session_messages, 24);
        assert_eq!(prompt.history.dropped_session_messages, 0);
        assert!(user_contents
            .iter()
            .any(|content| content.contains("用户轮次 0 的目标")));
        assert!(user_contents
            .iter()
            .any(|content| content.contains("继续第 12 轮")));
        assert_eq!(user_contents.len(), 12);
    }

    /** 已知小窗口必须丢掉最早历史，但当前用户消息和最近一轮仍在。 */
    #[test]
    fn model_messages_drop_oldest_history_when_context_window_is_small() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续");
        let available_skills = crate::skills::built_in_skills();
        let bulky = "这段历史需要占用预算。".repeat(40);
        snapshot.sessions[0].messages = (0..12)
            .flat_map(|index| {
                vec![
                    history_test_message(
                        &format!("user-{index}"),
                        "user",
                        &format!("用户 {index} {bulky}"),
                        None,
                        Vec::new(),
                    ),
                    history_test_message(
                        &format!("assistant-{index}"),
                        "assistant",
                        &format!("助手 {index} {bulky}"),
                        None,
                        Vec::new(),
                    ),
                ]
            })
            .collect();

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-11",
            &[],
            Some(4_096),
            None,
        );
        let user_contents = prompt
            .messages
            .iter()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>();

        assert!(prompt.history.dropped_session_messages > 0);
        assert!(prompt.history.included_session_messages >= MIN_RETAINED_SESSION_MESSAGES);
        assert!(
            prompt.history.used_chars <= prompt.history.budget_chars
                || prompt.history.included_session_messages == MIN_RETAINED_SESSION_MESSAGES
        );
        assert!(user_contents.iter().any(|content| content.contains("继续")));
        assert!(user_contents
            .iter()
            .all(|content| !content.contains("用户 0 ")));
    }

    /** 温窗口的旧工具结果只保留 status/summary，热窗口仍回放 result_preview。 */
    #[test]
    fn model_messages_collapse_warm_tool_results_but_keep_hot_previews() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续");
        let available_skills = crate::skills::built_in_skills();
        let mut messages = vec![AgentMessage {
            id: "assistant-old".to_owned(),
            role: "assistant".to_owned(),
            content: "先检索旧笔记。".to_owned(),
            action: Some("ask".to_owned()),
            citations: None,
            tool_calls: Some(vec![AgentToolCall {
                id: "call-search-old".to_owned(),
                name: "search_notes".to_owned(),
                status: "completed".to_owned(),
                summary: "已检索到旧笔记".to_owned(),
                args: json!({ "query": "旧笔记" }),
            }]),
            mentioned_file_ids: Vec::new(),
            trace: vec![AgentTraceStep {
                id: "trace-old".to_owned(),
                step_type: "tool".to_owned(),
                timestamp: "刚刚".to_owned(),
                content: None,
                name: Some("search_notes".to_owned()),
                status: Some("completed".to_owned()),
                summary: Some("已检索到旧笔记".to_owned()),
                args: Some(json!({ "query": "旧笔记" })),
                result_preview: Some(
                    r#"{"matches":[{"title":"旧笔记预览不应进入温窗口"}]}"#.to_owned(),
                ),
                error: None,
                duration_ms: Some(8),
            }],
            turn_duration_ms: Some(800),
        }];
        messages.extend((0..HOT_HISTORY_SESSION_MESSAGES).map(|index| {
            history_test_message(
                &format!("filler-{index}"),
                if index % 2 == 0 { "user" } else { "assistant" },
                &format!("填充消息 {index}"),
                None,
                Vec::new(),
            )
        }));
        messages.push(history_test_message(
            "user-current",
            "user",
            "继续",
            None,
            Vec::new(),
        ));
        snapshot.sessions[0].messages = messages;

        let packed = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            None,
            None,
        )
        .messages;
        let old_tool = packed
            .iter()
            .find(|message| message["tool_call_id"] == "call-search-old")
            .expect("warm history should still replay the old tool message");

        assert!(old_tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("已检索到旧笔记"));
        assert!(!old_tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("旧笔记预览不应进入温窗口"));
    }

    /** 未总结内容被预算裁掉时，应触发自动整理，把丢掉的历史写入工作记忆。 */
    #[test]
    fn context_summary_auto_decision_triggers_when_unsummarized_history_is_dropped() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.sessions[0].messages = (0..12)
            .map(|index| {
                history_test_message(
                    &format!("message-{index}"),
                    "user",
                    &format!("消息 {index}"),
                    None,
                    Vec::new(),
                )
            })
            .collect();
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-07-08 10:00:00".to_owned(),
            current_goal: Some("旧目标".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("message-1".to_owned()),
            last_compacted_message_id: Some("message-1".to_owned()),
        });

        let decision = context_summary_auto_decision(
            &snapshot.sessions[0],
            1_000,
            Some(&PackedHistoryStats {
                included_session_messages: 4,
                dropped_session_messages: 8,
                budget_chars: 4_000,
                used_chars: 3_900,
            }),
            None,
            None,
            false,
        );

        assert!(decision.should_compact);
        assert!(decision
            .reasons
            .contains(&"unsummarizedHistoryDropped".to_owned()));
    }

    /** 工作记忆请求必须带 json_schema，避免模型自由发挥后再靠 fence 抠 JSON。 */
    #[test]
    fn chat_completion_payload_includes_context_summary_json_schema() {
        let messages = vec![json!({ "role": "system", "content": "只输出 JSON" })];
        let payload = build_chat_completion_payload(
            "gpt-4o-mini",
            &messages,
            None,
            Some(&context_summary_response_format()),
            false,
        );
        let schema = &payload["response_format"];

        assert_eq!(schema["type"], "json_schema");
        assert_eq!(schema["json_schema"]["name"], "agent_context_summary");
        assert_eq!(schema["json_schema"]["strict"], true);
        assert_eq!(schema["json_schema"]["schema"]["type"], "object");
        assert!(schema["json_schema"]["schema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "currentGoal"));
        assert!(payload.get("tools").is_none());
    }

    /** 主 Agent loop 的 payload 不应误带 response_format，工具 schema 仍按原样发送。 */
    #[test]
    fn chat_completion_payload_keeps_tools_without_response_format() {
        let tools = json!([{ "type": "function", "function": { "name": "search_notes" } }]);
        let payload = build_chat_completion_payload(
            "gpt-4o-mini",
            &[json!({ "role": "user", "content": "检索" })],
            Some(&tools),
            None,
            false,
        );

        assert_eq!(payload["tool_choice"], "auto");
        assert_eq!(payload["tools"], tools);
        assert!(payload.get("response_format").is_none());
        assert!(payload.get("stream").is_none());
    }

    /** Agent loop 必须显式打开 Chat Completions SSE，不能靠事后打字机伪装流式。 */
    #[test]
    fn chat_completion_payload_enables_stream_for_agent_loop() {
        let payload = build_chat_completion_payload(
            "gpt-4o-mini",
            &[json!({ "role": "user", "content": "你好" })],
            None,
            None,
            true,
        );

        assert_eq!(payload["stream"], true);
        assert_eq!(payload["stream_options"]["include_usage"], true);
    }

    /** json_schema 成功时 content 仍是 JSON 字符串，解析层要直接读成工作记忆对象。 */
    #[test]
    fn parse_context_summary_response_reads_json_object_content() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "{\"version\":1,\"updatedAt\":\"2026-08-22 10:00:00\",\"currentGoal\":\"继续改写\",\"userConstraints\":[],\"decisions\":[],\"completedWork\":[\"已检索笔记\"],\"pendingTasks\":[],\"touchedNotes\":[],\"pendingChangeSummary\":null,\"openQuestions\":[],\"lastSummarizedMessageId\":\"assistant-old\",\"lastCompactedMessageId\":\"assistant-old\"}"
                }
            }]
        });
        let summary = parse_context_summary_response(response).expect("json content should parse");

        assert_eq!(summary.current_goal.as_deref(), Some("继续改写"));
        assert_eq!(summary.completed_work, vec!["已检索笔记".to_owned()]);
    }

    /** 部分兼容 provider 会把 integer 输出成字符串 "1"，不能因此丢掉整份工作记忆。 */
    #[test]
    fn parse_context_summary_response_accepts_string_version() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "{\"version\":\"1\",\"updatedAt\":\"2026-08-24 19:02:03\",\"currentGoal\":\"记住苹果是红色的\",\"userConstraints\":[],\"decisions\":[],\"completedWork\":[],\"pendingTasks\":[],\"touchedNotes\":[],\"pendingChangeSummary\":null,\"openQuestions\":[],\"lastSummarizedMessageId\":null,\"lastCompactedMessageId\":null}"
                }
            }]
        });
        let summary =
            parse_context_summary_response(response).expect("string version should coerce to u32");

        assert_eq!(summary.version, 1);
        assert_eq!(summary.current_goal.as_deref(), Some("记住苹果是红色的"));
    }

    /** 已启用的跨会话记忆应作为独立 system 层注入，且位于项目指令之后、会话工作记忆之前。 */
    #[test]
    fn kb_memory_injected_between_project_instructions_and_session_summary() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结当前偏好");
        let available_skills = crate::skills::built_in_skills();
        let kb_memories = vec![KnowledgeBaseMemory {
            knowledge_base_id: "kb-a".to_owned(),
            enabled: true,
            entries: vec![AgentMemoryEntry {
                id: "mem-1".to_owned(),
                category: "tagConvention".to_owned(),
                content: "标签统一使用小写连字符".to_owned(),
                source: "user".to_owned(),
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            updated_at: "刚刚".to_owned(),
        }];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &kb_memories,
        );

        // 索引 0 是主 system；注入的记忆层应包含【跨会话记忆】头部和脱敏后的条目内容。
        let memory_content = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .find(|content| content.contains("【跨会话记忆】"))
            .unwrap_or_default();
        assert!(memory_content.contains("【跨会话记忆】"));
        assert!(memory_content.contains("标签规范"));
        assert!(memory_content.contains("标签统一使用小写连字符"));
    }

    /** 知识库根目录 AGENTS.md 必须强制注入唯一 system，不能指望模型自己去读。 */
    #[test]
    fn agents_md_is_injected_into_unique_system_prompt() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "标签必须使用小写连字符。").unwrap();
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = dir.path().to_string_lossy().into_owned();
        let request = runtime_test_request("ask", "总结当前偏好");
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &crate::skills::built_in_skills(),
            &[],
            "user-current",
            &[],
        );

        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.contains("<project_context>"));
        assert!(system.contains("【项目级 Agent 指令】"));
        assert!(system.contains("AGENTS.md"));
        assert!(system.contains("标签必须使用小写连字符。"));
        assert!(system.contains("主知识库"));
        let project_index = system.find("【项目级 Agent 指令】").unwrap();
        let memory_index = system.find("【范围】").unwrap();
        assert!(project_index < memory_index);
    }

    /** 没有 AGENTS.md 时，旧的 ORANGE_AGENT.md 仍应作为兼容回退注入。 */
    #[test]
    fn orange_agent_md_is_injected_when_agents_md_is_absent() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ORANGE_AGENT.md"), "兼容旧说明书。").unwrap();
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = dir.path().to_string_lossy().into_owned();
        let request = runtime_test_request("ask", "总结");
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &crate::skills::built_in_skills(),
            &[],
            "user-current",
            &[],
        );

        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.contains("ORANGE_AGENT.md"));
        assert!(system.contains("兼容旧说明书。"));
    }

    /** 两份都在时只注入 AGENTS.md，避免项目规则重复占用 system。 */
    #[test]
    fn agents_md_wins_over_legacy_orange_agent_md() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "标准说明书。").unwrap();
        fs::write(dir.path().join("ORANGE_AGENT.md"), "旧说明书不应出现。").unwrap();
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = dir.path().to_string_lossy().into_owned();
        let request = runtime_test_request("ask", "总结");
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &crate::skills::built_in_skills(),
            &[],
            "user-current",
            &[],
        );

        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.contains("标准说明书。"));
        assert!(!system.contains("旧说明书不应出现。"));
    }

    /** 未授权知识库的说明书不得进入当前会话的 system。 */
    #[test]
    fn unauthorized_knowledge_base_instruction_is_not_injected() {
        let authorized = tempdir().unwrap();
        let unauthorized = tempdir().unwrap();
        fs::write(authorized.path().join("AGENTS.md"), "授权库规则。").unwrap();
        fs::write(unauthorized.path().join("AGENTS.md"), "未授权库规则。").unwrap();
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = authorized.path().to_string_lossy().into_owned();
        snapshot.knowledge_bases[1].path = unauthorized.path().to_string_lossy().into_owned();
        let request = runtime_test_request("ask", "总结");
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &crate::skills::built_in_skills(),
            &[],
            "user-current",
            &[],
        );

        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.contains("授权库规则。"));
        assert!(!system.contains("未授权库规则。"));
    }

    /** 根目录说明书已在 system 中时，本轮 @ 不再重复贴全文。 */
    #[test]
    fn mentioned_project_instruction_is_not_duplicated_in_user_message() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "不要重复注入。").unwrap();
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = dir.path().to_string_lossy().into_owned();
        snapshot.notes.push(Note {
            id: "note-agents".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "Agent 说明书".to_owned(),
            path: "AGENTS.md".to_owned(),
            content_hash: hash_content("不要重复注入。"),
            content: "不要重复注入。".to_owned(),
            tags: Vec::new(),
            updated_at: "刚刚".to_owned(),
            backlinks: Vec::new(),
        });
        let mut request = runtime_test_request("ask", "按说明书整理");
        request.mentioned_file_ids = vec!["note-agents".to_owned()];
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &crate::skills::built_in_skills(),
            &[],
            "user-current",
            &[],
        );

        let system = messages[0]["content"].as_str().unwrap_or_default();
        let user = messages
            .last()
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default();
        assert!(system.contains("不要重复注入。"));
        assert!(!user.contains("【本轮用户显式 @ 的文件】"));
    }

    /** 跨会话记忆注入模型前必须再次脱敏，防止旧数据绕过保存入口。 */
    #[test]
    fn kb_memory_prompt_redacts_secrets_before_model_context() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结当前偏好");
        let available_skills = crate::skills::built_in_skills();
        let kb_memories = vec![KnowledgeBaseMemory {
            knowledge_base_id: "kb-a".to_owned(),
            enabled: true,
            entries: vec![AgentMemoryEntry {
                id: "mem-1".to_owned(),
                category: "unknownCategory".to_owned(),
                content: "固定偏好里误写了手机号 13800138000 和 api_key=ak_live_12345678"
                    .to_owned(),
                source: "user".to_owned(),
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            updated_at: "刚刚".to_owned(),
        }];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &kb_memories,
        );

        let memory_content = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .find(|content| content.contains("【跨会话记忆】"))
            .unwrap_or_default();

        assert!(memory_content.contains("[已脱敏]"));
        assert!(memory_content.contains("其他偏好"));
        assert!(!memory_content.contains("13800138000"));
        assert!(!memory_content.contains("ak_live_12345678"));
        assert!(!memory_content.contains("unknownCategory"));
    }

    /** 未启用或空条目的跨会话记忆不应注入任何 system 层。 */
    #[test]
    fn disabled_or_empty_kb_memory_not_injected() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结");
        let available_skills = crate::skills::built_in_skills();

        // enabled=false 不注入。
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[KnowledgeBaseMemory {
                knowledge_base_id: "kb-a".to_owned(),
                enabled: false,
                entries: vec![AgentMemoryEntry {
                    id: "mem-1".to_owned(),
                    category: "other".to_owned(),
                    content: "不应出现".to_owned(),
                    source: "user".to_owned(),
                    created_at: "刚刚".to_owned(),
                    updated_at: "刚刚".to_owned(),
                }],
                updated_at: "刚刚".to_owned(),
            }],
        );
        assert!(messages.iter().all(|message| {
            message["content"]
                .as_str()
                .map(|content| !content.contains("【跨会话记忆】"))
                .unwrap_or(true)
        }));

        // 空条目不注入。
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[KnowledgeBaseMemory {
                knowledge_base_id: "kb-a".to_owned(),
                enabled: true,
                entries: Vec::new(),
                updated_at: "刚刚".to_owned(),
            }],
        );
        assert!(messages.iter().all(|message| {
            message["content"]
                .as_str()
                .map(|content| !content.contains("【跨会话记忆】"))
                .unwrap_or(true)
        }));
    }

    /** 超长跨会话记忆渲染时应被截断到预算上限内。 */
    #[test]
    fn kb_memory_prompt_truncated_within_budget() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结");
        let available_skills = crate::skills::built_in_skills();
        // 单条内容远超预算上限，强制触发截断。
        let long_content = "标签偏好：".to_owned() + &"标签详细说明".repeat(2000);
        let kb_memories = vec![KnowledgeBaseMemory {
            knowledge_base_id: "kb-a".to_owned(),
            enabled: true,
            entries: vec![AgentMemoryEntry {
                id: "mem-1".to_owned(),
                category: "tagConvention".to_owned(),
                content: long_content,
                source: "user".to_owned(),
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            updated_at: "刚刚".to_owned(),
        }];

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &kb_memories,
        );

        let system_content = messages[0]["content"].as_str().unwrap_or_default();
        let memory_start = system_content.find("【跨会话记忆】").unwrap_or(0);
        let memory_content = &system_content[memory_start..];
        let memory_end = memory_content
            .find("\n\n【")
            .unwrap_or(memory_content.len());
        let memory_section = &memory_content[..memory_end];
        assert!(!memory_section.is_empty());
        assert!(
            memory_section.chars().count() <= MAX_RENDERED_KB_MEMORY_CHARS + 200,
            "跨会话记忆渲染应被截断到预算上限附近，实际 {} 字符",
            memory_section.chars().count()
        );
    }

    /** RequestAuditLog 只记录工作记忆注入和更新后的长度/时间，不保存 summary 正文。 */
    #[test]
    fn audit_log_records_context_summary_metrics_without_body() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let mut audit_trail = RuntimeAuditTrail::default();

        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-07-08 10:00:00".to_owned(),
            current_goal: Some("敏感目标正文不应进入审计".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: None,
            last_compacted_message_id: None,
        });
        audit_trail.record_context_summary_injection(&snapshot.sessions[0]);

        let audit_log = build_audit_log(
            "model_turn",
            &snapshot,
            0,
            "用户输入",
            "OpenAI-compatible 模型请求",
            &audit_trail,
        );

        assert!(audit_log
            .content_summary
            .contains("工作记忆：injected=true"));
        assert!(audit_log.content_summary.contains("stored=true"));
        assert!(audit_log
            .content_summary
            .contains("injected_updated_at=2026-07-08 10:00:00"));
        assert!(!audit_log.content_summary.contains("敏感目标正文"));
    }

    /** 自动整理会按消息数、未 compact 消息数、prompt 预算和 pending diff 变化触发。 */
    #[test]
    fn context_summary_auto_decision_reports_triggers() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        snapshot.sessions[0].messages = (0..30)
            .map(|index| AgentMessage {
                id: format!("message-{index}"),
                role: if index % 2 == 0 { "user" } else { "assistant" }.to_owned(),
                content: format!("消息 {index}"),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: None,
                mentioned_file_ids: Vec::new(),
                trace: Vec::new(),
                turn_duration_ms: None,
            })
            .collect();
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-07-08 10:00:00".to_owned(),
            current_goal: Some("旧目标".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("message-29".to_owned()),
            last_compacted_message_id: Some("message-0".to_owned()),
        });
        snapshot.sessions[0].pending_change = Some(runtime_test_pending_change("pending"));

        let decision = context_summary_auto_decision(
            &snapshot.sessions[0],
            AUTO_COMPACT_PROMPT_CHAR_THRESHOLD + 1,
            None,
            None,
            None,
            false,
        );

        assert!(decision.should_compact);
        assert!(decision
            .reasons
            .contains(&"unsummarizedMessagesOverThreshold".to_owned()));
        assert!(decision
            .reasons
            .contains(&"promptCharsOverThreshold".to_owned()));
        assert!(!decision
            .reasons
            .contains(&"pendingChangeChanged".to_owned()));
        assert!(!decision.reasons.contains(&"firstSummary".to_owned()));
        assert_eq!(decision.unsummarized_message_count, 29);
    }

    /** 短会话不得因为「还没有工作记忆」就 compact；检查点会打断 cache、也让模型误以为被压缩。 */
    #[test]
    fn context_summary_auto_decision_skips_first_short_turn() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.sessions[0].messages = vec![
            history_test_message("user-1", "user", "你能做什么", None, Vec::new()),
            history_test_message(
                "assistant-1",
                "assistant",
                "我是橘记 Agent。",
                None,
                Vec::new(),
            ),
        ];

        let decision = context_summary_auto_decision(
            &snapshot.sessions[0],
            2_000,
            None,
            Some("glm-5.2"),
            None,
            false,
        );

        assert!(!decision.should_compact);
        assert!(decision.reasons.is_empty());
    }

    /** 增量 summary 请求从上次模型 compact 后截取消息，而不是被每轮确定性同步重置。 */
    #[test]
    fn context_summary_recent_payload_uses_last_compacted_marker() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        snapshot.sessions[0].messages = (0..6)
            .map(|index| AgentMessage {
                id: format!("message-{index}"),
                role: "user".to_owned(),
                content: format!("消息正文 {index}"),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: None,
                mentioned_file_ids: Vec::new(),
                trace: Vec::new(),
                turn_duration_ms: None,
            })
            .collect();
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-07-08 10:00:00".to_owned(),
            current_goal: Some("旧目标".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("message-5".to_owned()),
            last_compacted_message_id: Some("message-2".to_owned()),
        });

        let payload = context_summary_recent_message_payload(&snapshot.sessions[0]);

        assert!(!payload.contains("message-2"));
        assert!(payload.contains("message-3"));
        assert!(payload.contains("message-5"));
    }

    /** 模型启用后的配置或请求错误必须进入可见会话消息，不能静默伪装成本地规则回答。 */
    #[test]
    fn model_error_turn_records_visible_failed_model_request() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "普通问题");
        let provider = runtime_test_provider();
        let available_skills = crate::skills::built_in_skills();
        let result = model_error_turn(
            snapshot,
            request,
            Some(&provider),
            Some("selected-model"),
            &available_skills,
            &[],
            "模型请求失败：测试错误",
            None,
        );
        let session = &result.turn_result.snapshot.sessions[0];
        let last_message = session.messages.last().unwrap();
        let tool_calls = last_message.tool_calls.as_ref().unwrap();
        let tool_call = tool_calls.last().unwrap();

        assert_eq!(result.audit_log.kind, "model_error_turn");
        assert!(last_message.content.contains("真实模型请求没有完成"));
        assert_eq!(tool_calls.first().unwrap().name, "skill_context");
        assert_eq!(tool_call.name, "model_request");
        assert_eq!(tool_call.status, "failed");
        assert_eq!(tool_call.args["model"], "selected-model");
        assert_eq!(tool_call.args["providerId"], provider.id);
        assert!(tool_call.args.get("apiKey").is_none());
    }

    /** provider 解析失败（例如未找到 provider）时也必须返回可见错误，而不是 panic 或静默降级。 */
    #[test]
    fn model_error_turn_without_provider_still_records_visible_error() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "普通问题");
        let available_skills = crate::skills::built_in_skills();
        let result = model_error_turn(
            snapshot,
            request,
            None,
            None,
            &available_skills,
            &[],
            "未找到 Provider 配置：missing-provider",
            None,
        );
        let session = &result.turn_result.snapshot.sessions[0];
        let last_message = session.messages.last().unwrap();

        assert!(last_message.content.contains("真实模型请求没有完成"));
        assert!(last_message.content.contains("missing-provider"));
    }

    /** 显式 Skill 全文在最后一条 user，目录仍在唯一 system。 */
    #[test]
    fn model_messages_include_explicit_skill_instructions() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "按研究流程总结");
        let available_skills = crate::skills::built_in_skills();
        let explicit_skill = available_skills
            .iter()
            .find(|skill| skill.id == "skill-note-research")
            .unwrap()
            .clone();
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            std::slice::from_ref(&explicit_skill),
            "user-current",
            &[],
        );
        let system_content = messages[0]["content"].as_str().unwrap_or_default();
        let last_user = messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .unwrap()["content"]
            .as_str()
            .unwrap_or_default();

        assert!(system_content.contains("<available_skills>"));
        assert!(system_content.contains("不能扩大工具权限"));
        assert!(!system_content.contains(&explicit_skill.instructions));
        assert!(!system_content.contains("本轮显式激活的 Skills"));
        assert!(last_user.contains("<skill name=\"note-research\""));
        assert!(last_user.contains(&explicit_skill.instructions));
        assert!(last_user.contains("按研究流程总结"));
    }

    /** 安全等级指令应讲放手和确认策略，而不是把级别定义成 Skill 开关。 */
    #[test]
    fn model_messages_describe_security_level_as_autonomy() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结当前笔记");
        let available_skills = crate::skills::built_in_skills();

        let basic = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let basic_content = basic[0]["content"].as_str().unwrap_or_default();
        assert!(basic_content.contains("当前为基础级别"));
        assert!(basic_content.contains("先看紧"));
        assert!(basic_content.contains("待确认 diff"));

        snapshot.sessions[0].security_level = "advanced".to_owned();
        let advanced = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let advanced_content = advanced[0]["content"].as_str().unwrap_or_default();
        assert!(advanced_content.contains("当前为进阶级别"));
        assert!(advanced_content.contains("开始放手"));
        assert!(advanced_content.contains("仍需用户确认"));

        snapshot.sessions[0].security_level = "autonomous".to_owned();
        let autonomous = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let autonomous_content = autonomous[0]["content"].as_str().unwrap_or_default();
        assert!(autonomous_content.contains("当前为完全级别"));
        assert!(autonomous_content.contains("连续执行权"));
        assert!(autonomous_content.contains("自动落盘"));
        assert!(autonomous_content.contains("Skill 只是更高权限下可发挥的能力之一"));
        assert!(autonomous_content.contains("不是整台电脑"));
        assert!(!autonomous_content.contains("list_path"));
        assert!(!autonomous_content.contains("create_folder"));
        assert!(!autonomous_content.contains("生成待确认 diff，不能声称已经写入文件"));
    }

    /** summary-only 请求要显式带上本轮失败摘要，避免工具失败只藏在最近消息里。 */
    #[test]
    fn context_summary_model_messages_include_turn_failure_summary() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let messages = build_context_summary_model_messages(
            &snapshot.sessions[0],
            Some("read_note 工具失败：目标笔记不在 scope 内"),
        );
        let user_content = messages[1]["content"].as_str().unwrap_or_default();

        assert!(user_content.contains("本轮失败摘要"));
        assert!(user_content.contains("目标笔记不在 scope 内"));
    }

    /** 待确认 diff 只在 pending 状态进入模型 prompt，accepted/rejected 不再伪装成当前待确认变更。 */
    #[test]
    fn pending_change_prompt_only_includes_pending_status() {
        let pending = runtime_test_pending_change("pending");
        let accepted = runtime_test_pending_change("accepted");

        assert!(render_pending_change_prompt(Some(&pending))
            .unwrap()
            .contains("状态：pending"));
        assert!(render_pending_change_prompt(Some(&accepted)).is_none());
    }

    /** 确定性 summary fallback 要保留失败原因和 pending diff 摘要，但不写入完整正文。 */
    #[test]
    fn deterministic_context_summary_records_failure_and_pending_change() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        snapshot.sessions[0].messages.push(AgentMessage {
            id: "user-a".to_owned(),
            role: "user".to_owned(),
            content: "继续生成 diff".to_owned(),
            action: Some("rewrite".to_owned()),
            citations: None,
            tool_calls: None,
            mentioned_file_ids: Vec::new(),
            trace: Vec::new(),
            turn_duration_ms: None,
        });
        snapshot.sessions[0].pending_change = Some(runtime_test_pending_change("pending"));

        update_agent_context_summary_deterministic(
            &mut snapshot,
            0,
            Some("read_note 工具失败：目标笔记不在 scope 内"),
            false,
        );

        let summary = snapshot.sessions[0].context_summary.as_ref().unwrap();
        let rendered = render_context_summary_body(summary).unwrap_or_default();

        assert!(summary
            .pending_tasks
            .iter()
            .any(|task| task.contains("目标笔记不在 scope 内")));
        assert!(summary
            .pending_change_summary
            .as_deref()
            .unwrap_or_default()
            .contains("状态：pending"));
        assert!(rendered.contains("addedLines=2"));
        assert!(!rendered.contains("旧正文里有较长内容"));
        assert!(!rendered.contains("新正文里有较长内容"));
    }

    /** resolve_explicit_skills 会按选择顺序去重、限制数量并拒绝已禁用 Skill。 */
    #[test]
    fn resolve_explicit_skills_dedupes_limits_and_rejects_disabled() {
        let mut available_skills = crate::skills::built_in_skills();
        let ids = vec![
            "skill-note-research".to_owned(),
            "skill-note-research".to_owned(),
            "skill-note-rewrite".to_owned(),
            "skill-draft-from-context".to_owned(),
            "skill-organize-knowledge".to_owned(),
        ];
        let selection = resolve_explicit_skills(&ids, &available_skills).unwrap();

        assert_eq!(selection.skills.len(), MAX_EXPLICIT_SKILLS_PER_TURN);
        assert_eq!(selection.skills[0].id, "skill-note-research");
        assert_eq!(selection.skills[1].id, "skill-note-rewrite");
        assert!(selection.truncated);

        available_skills[0].enabled = false;
        let error = resolve_explicit_skills(&["skill-note-research".to_owned()], &available_skills)
            .unwrap_err();

        assert!(error.contains("已禁用"));
    }

    /** 显式 Skill 缺失时返回可见错误，并记录 failed activate_skill。 */
    #[test]
    fn skill_activation_error_turn_records_failed_activate_skill_for_missing_skill() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let mut request = runtime_test_request("ask", "按不存在的 skill 执行");
        let available_skills = crate::skills::built_in_skills();

        request.explicit_skill_ids = vec!["missing-skill".to_owned()];
        let result = skill_activation_error_turn(
            snapshot,
            request,
            &available_skills,
            &[],
            "显式选择的 Skill 不存在或已被移除：missing-skill",
        );
        let session = &result.turn_result.snapshot.sessions[0];
        let last_message = session.messages.last().unwrap();
        let tool_calls = last_message.tool_calls.as_ref().unwrap();
        let activate_call = tool_calls
            .iter()
            .find(|tool_call| tool_call.name == "activate_skill")
            .unwrap();

        assert_eq!(result.audit_log.kind, "skill_activation_error_turn");
        assert!(last_message.content.contains("显式 Skill 未完成执行"));
        assert_eq!(activate_call.status, "failed");
        assert_eq!(
            activate_call.args["skillIds"][0].as_str(),
            Some("missing-skill")
        );
        assert!(!activate_call.args.to_string().contains("当用户要求查找"));
    }

    /** 已选 Skill 但真实模型 turn 不可执行时，不应伪装成本地规则 Agent。 */
    #[test]
    fn explicit_skill_error_turn_does_not_use_local_rule_fallback() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let mut request = runtime_test_request("ask", "按研究流程总结");
        let available_skills = crate::skills::built_in_skills();
        let explicit_skill = available_skills
            .iter()
            .find(|skill| skill.id == "skill-note-research")
            .unwrap()
            .clone();

        request.explicit_skill_ids = vec![explicit_skill.id.clone()];
        let result = skill_activation_error_turn(
            snapshot,
            request,
            &available_skills,
            std::slice::from_ref(&explicit_skill),
            "已显式选择 Skill，但当前模型未启用，无法执行 strict skill turn。",
        );
        let tool_calls = result.turn_result.snapshot.sessions[0]
            .messages
            .last()
            .unwrap()
            .tool_calls
            .as_ref()
            .unwrap();
        let activate_call = tool_calls
            .iter()
            .find(|tool_call| tool_call.name == "activate_skill")
            .unwrap();

        assert!(tool_calls
            .iter()
            .all(|tool_call| tool_call.name != "local_rule_agent"));
        assert_eq!(activate_call.status, "failed");
        assert!(tool_calls
            .iter()
            .any(|tool_call| tool_call.name == "model_request" && tool_call.status == "failed"));
        assert!(result
            .audit_log
            .content_summary
            .contains("显式 Skill：1 个"));
    }

    /** activate_skill 轨迹只能包含元数据，不能把完整 instructions 暴露到 UI 或审计。 */
    #[test]
    fn activate_skill_tool_call_omits_instructions() {
        let available_skills = crate::skills::built_in_skills();
        let explicit_skill = available_skills
            .iter()
            .find(|skill| skill.id == "skill-note-research")
            .unwrap()
            .clone();
        let calls =
            activate_skill_tool_calls(std::slice::from_ref(&explicit_skill), "completed", None);
        let call = calls.first().unwrap();
        let serialized_args = call.args.to_string();

        assert_eq!(call.name, "activate_skill");
        assert_eq!(call.status, "completed");
        assert_eq!(
            call.args["skillId"].as_str(),
            Some(explicit_skill.id.as_str())
        );
        assert_eq!(
            call.args["instructionChars"].as_u64(),
            Some(explicit_skill.instructions.chars().count() as u64)
        );
        assert!(!serialized_args.contains(&explicit_skill.instructions));
        assert!(!call.summary.contains(&explicit_skill.instructions));
    }

    /** 本轮显式选择了 providerId 时，必须记为会话默认，下次打开该会话选择器才能展示“最后一次切换”的模型。 */
    #[test]
    fn remember_requested_provider_on_session_updates_session_when_explicitly_selected() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        remember_requested_provider_on_session(
            &mut snapshot.sessions[0],
            Some("provider-b"),
            Some("model-b"),
            "model-b",
        );

        assert_eq!(
            snapshot.sessions[0].model_provider_id,
            Some("provider-b".to_owned())
        );
        assert_eq!(snapshot.sessions[0].model_id, Some("model-b".to_owned()));
    }

    /** 本轮没有显式选择 providerId 时，不能改动会话已有设置，否则会话会被意外固定到当前全局默认 provider。 */
    #[test]
    fn remember_requested_provider_on_session_keeps_session_unchanged_without_explicit_selection() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        remember_requested_provider_on_session(&mut snapshot.sessions[0], None, None, "test-model");
        assert_eq!(snapshot.sessions[0].model_provider_id, None);
        assert_eq!(snapshot.sessions[0].model_id, None);

        remember_requested_provider_on_session(
            &mut snapshot.sessions[0],
            Some("   "),
            None,
            "test-model",
        );
        assert_eq!(snapshot.sessions[0].model_provider_id, None);
        assert_eq!(snapshot.sessions[0].model_id, None);
    }

    /** 模型最终回答不能绕过工具系统自动生成 pending diff。 */
    #[test]
    fn assistant_message_without_write_tool_does_not_create_pending_change() {
        let mut snapshot = runtime_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = runtime_test_request("rewrite", "请改写当前笔记");

        let tracer = AgentTurnTracer::new("session-a", "assistant-test");
        push_assistant_message(
            &mut snapshot,
            0,
            &request.action,
            "模型直接返回的改写正文".to_owned(),
            Vec::new(),
            Vec::new(),
            &tracer,
        );

        assert!(snapshot.sessions[0].pending_change.is_none());
    }

    /** DeepSeek 风格 DSML 工具调用应被解析为真实工具调用，并从用户可见正文中移除。 */
    #[test]
    fn dsml_tool_call_text_is_parsed_and_stripped() {
        let message = json!({
            "role": "assistant",
            "content": "先生成第一处去重。<｜｜DSML｜｜tool_calls><｜｜DSML｜｜invoke name=\"propose_note_change\"><｜｜DSML｜｜parameter name=\"noteId\" string=\"true\">note-a</｜｜DSML｜｜parameter><｜｜DSML｜｜parameter name=\"operation\" string=\"true\">replace</｜｜DSML｜｜parameter><｜｜DSML｜｜parameter name=\"original\" string=\"true\">旧段落</｜｜DSML｜｜parameter><｜｜DSML｜｜parameter name=\"next\" string=\"true\">新段落</｜｜DSML｜｜parameter></｜｜DSML｜｜invoke></｜｜DSML｜｜tool_calls>"
        });
        let extraction = extract_tool_calls_from_message(&message);
        let tool_call = extraction.tool_calls.first().unwrap();
        let args: Value = serde_json::from_str(
            tool_call["function"]["arguments"]
                .as_str()
                .unwrap_or_default(),
        )
        .unwrap();

        assert_eq!(extraction.visible_content, "先生成第一处去重。");
        assert_eq!(tool_call["function"]["name"], "propose_note_change");
        assert_eq!(args["noteId"], "note-a");
        assert_eq!(args["operation"], "replace");
        assert_eq!(args["original"], "旧段落");
        assert_eq!(args["next"], "新段落");
    }

    /** 工具失败后模型若仍输出成功话术，运行时必须改成失败说明。 */
    #[test]
    fn final_content_success_claim_is_overridden_after_tool_failure() {
        let content = reconcile_final_content_with_tool_status(
            "✅ 去重变更已生成！".to_owned(),
            Some("多处编辑第 1 处 original 在目标笔记中出现多次，请提供更长、更唯一的片段。"),
        );

        assert!(content.contains("这次变更没有生成成功"));
        assert!(!content.contains("✅ 去重变更已生成"));
    }

    /** 已有 transcript 时必须原样追加，不能从 UI 会话记录重装并折叠工具结果。 */
    #[test]
    fn model_messages_append_previous_transcript_keeps_full_tool_result() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续刚才的检索");
        let available_skills = crate::skills::built_in_skills();
        let full_tool_result = r#"{"matches":[{"title":"隐私边界完整正文不应被折叠","score":0.9,"snippet":"很长的检索命中正文"}]}"#;
        let previous_transcript = vec![
            json!({
                "role": "user",
                "content": "界面 action 提示：ask\n用户输入：帮我找隐私边界"
            }),
            json!({
                "role": "assistant",
                "content": "我先检索相关笔记。",
                "tool_calls": [{
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "search",
                        "arguments": "{\"query\":\"隐私边界\"}"
                    }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "call_abc123",
                "content": full_tool_result
            }),
            json!({
                "role": "assistant",
                "content": "找到相关笔记了。"
            }),
        ];
        snapshot.sessions[0].messages = vec![
            history_test_message("user-old", "user", "帮我找隐私边界", None, Vec::new()),
            AgentMessage {
                id: "assistant-old".to_owned(),
                role: "assistant".to_owned(),
                content: "找到相关笔记了。".to_owned(),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: Some(vec![AgentToolCall {
                    id: "tool-host-id".to_owned(),
                    name: "search_notes".to_owned(),
                    status: "completed".to_owned(),
                    summary: "已检索到 1 条笔记".to_owned(),
                    args: json!({ "query": "隐私边界" }),
                }]),
                mentioned_file_ids: Vec::new(),
                trace: vec![AgentTraceStep {
                    id: "trace-search".to_owned(),
                    step_type: "tool".to_owned(),
                    timestamp: "刚刚".to_owned(),
                    content: None,
                    name: Some("search_notes".to_owned()),
                    status: Some("completed".to_owned()),
                    summary: Some("已检索到 1 条笔记".to_owned()),
                    args: Some(json!({ "query": "隐私边界" })),
                    result_preview: Some("truncated-preview".to_owned()),
                    error: None,
                    duration_ms: Some(12),
                }],
                turn_duration_ms: Some(1200),
            },
            history_test_message("user-current", "user", "继续刚才的检索", None, Vec::new()),
        ];

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            None,
            Some(&previous_transcript),
        );
        let tool = prompt
            .messages
            .iter()
            .find(|message| message["tool_call_id"] == "call_abc123")
            .expect("appended transcript should keep the original tool call id");
        let last_user = prompt
            .messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .expect("new user turn should be appended");

        assert_eq!(tool["content"], full_tool_result);
        assert!(prompt
            .messages
            .iter()
            .all(|message| message["tool_call_id"] != "tool-host-id"));
        assert!(!tool["content"]
            .as_str()
            .unwrap_or_default()
            .contains("truncated-preview"));
        assert!(last_user["content"]
            .as_str()
            .unwrap_or_default()
            .contains("继续刚才的检索"));
        assert_eq!(
            conversation_from_model_messages(&prompt.messages, prompt.prefix_len).len(),
            previous_transcript.len() + 1
        );
    }

    /** 切 transcript 时必须丢掉重建的 system 前缀，只保留对话消息。 */
    #[test]
    fn conversation_from_model_messages_strips_system_prefix() {
        let messages = vec![
            json!({ "role": "system", "content": "角色与工具" }),
            json!({ "role": "system", "content": "工作记忆" }),
            json!({ "role": "user", "content": "你好" }),
            json!({ "role": "assistant", "content": "你好，需要我做什么？" }),
        ];

        assert_eq!(
            conversation_from_model_messages(&messages, 1),
            vec![
                json!({ "role": "system", "content": "工作记忆" }),
                json!({ "role": "user", "content": "你好" }),
                json!({ "role": "assistant", "content": "你好，需要我做什么？" }),
            ]
        );
    }

    /** @ 材料应写进本轮 user 消息，而不是下一轮还会重注入的 system。 */
    #[test]
    fn mentioned_files_attach_to_current_user_message_when_appending() {
        let snapshot = runtime_test_snapshot("授权 Markdown 正文".to_owned());
        let mut request = runtime_test_request("ask", "参考这份材料继续");
        request.mentioned_file_ids = vec!["note-a".to_owned()];
        let available_skills = crate::skills::built_in_skills();
        let previous_transcript = vec![json!({
            "role": "user",
            "content": "界面 action 提示：ask\n用户输入：上一轮问题"
        })];

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            None,
            Some(&previous_transcript),
        );
        let last_user = prompt
            .messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .unwrap();
        let system_has_mention = prompt.messages.iter().any(|message| {
            message["role"] == "system"
                && message["content"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("【本轮用户显式 @ 的文件】")
        });

        assert!(last_user["content"]
            .as_str()
            .unwrap_or_default()
            .contains("授权 Markdown 正文"));
        assert!(!system_has_mention);
    }

    /** seed 路径有 last_compacted_message_id 时，不把切点之前的 user 正文装箱回来。 */
    #[test]
    fn seed_history_skips_messages_before_last_compacted_id() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "继续");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![
            history_test_message(
                "user-old",
                "user",
                &"压缩前的旧目标不应出现".repeat(400),
                None,
                Vec::new(),
            ),
            history_test_message("assistant-old", "assistant", "旧回复", None, Vec::new()),
            history_test_message("user-current", "user", "继续", None, Vec::new()),
        ];
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-08-26 10:00:00".to_owned(),
            current_goal: Some("继续整理".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("assistant-old".to_owned()),
            last_compacted_message_id: Some("assistant-old".to_owned()),
        });

        let messages = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            Some(4_096),
            None,
        )
        .messages;
        let joined = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!joined.contains("压缩前的旧目标不应出现"));
        assert!(joined.contains("压缩检查点"));
        assert!(joined.contains("继续"));
    }

    /** 仅有 UI 工作记忆、尚未 compact 时，不得把检查点伪装成 user 发给模型。 */
    #[test]
    fn seed_history_does_not_inject_checkpoint_before_compact() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "下次再找你吧");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![
            history_test_message("user-1", "user", "你能做什么", None, Vec::new()),
            history_test_message(
                "assistant-1",
                "assistant",
                "我可以检索和改写笔记。",
                None,
                Vec::new(),
            ),
            history_test_message("user-current", "user", "下次再找你吧", None, Vec::new()),
        ];
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-08-28 19:30:00".to_owned(),
            current_goal: Some("向用户介绍橘记 Agent 的能力范围".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: vec!["本轮回复：我可以检索和改写笔记。".to_owned()],
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("assistant-1".to_owned()),
            last_compacted_message_id: Some("assistant-1".to_owned()),
        });

        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let joined = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let assistant_count = messages
            .iter()
            .filter(|message| message["role"] == "assistant")
            .count();

        assert!(!joined.contains("压缩检查点"));
        assert!(joined.contains("你能做什么"));
        assert!(joined.contains("我可以检索和改写笔记。"));
        assert_eq!(assistant_count, 1);
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
    }

    /** 有 transcript 时不把压缩点之前的 session.messages 装箱回来。 */
    #[test]
    fn transcript_projection_ignores_pre_checkpoint_session_messages() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "下一问");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![history_test_message(
            "user-old",
            "user",
            "这段 UI 旧消息不应进入 LLM",
            None,
            Vec::new(),
        )];
        let previous_transcript = vec![json!({
            "role": "user",
            "content": "界面 action 提示：ask\n用户输入：检查点之后的问题"
        })];

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            None,
            Some(&previous_transcript),
        );
        let joined = prompt
            .messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!joined.contains("这段 UI 旧消息不应进入 LLM"));
        assert!(joined.contains("检查点之后的问题"));
        assert!(joined.contains("下一问"));
    }

    /** transcript 若只有 user、没有 assistant，必须回退到 session.messages，避免模型看不到自己的回复。 */
    #[test]
    fn unusable_transcript_falls_back_to_session_history_with_assistant() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "下次再找你吧");
        let available_skills = crate::skills::built_in_skills();
        snapshot.sessions[0].messages = vec![
            history_test_message("user-1", "user", "你能做什么", None, Vec::new()),
            history_test_message(
                "assistant-1",
                "assistant",
                "我是橘记的本地优先知识库 Agent。",
                None,
                Vec::new(),
            ),
            history_test_message("user-current", "user", "下次再找你吧", None, Vec::new()),
        ];
        snapshot.sessions[0].context_summary = Some(AgentContextSummary {
            version: 1,
            updated_at: "2026-08-28 19:30:00".to_owned(),
            current_goal: Some("向用户介绍橘记 Agent 的能力范围".to_owned()),
            user_constraints: Vec::new(),
            decisions: Vec::new(),
            completed_work: Vec::new(),
            pending_tasks: Vec::new(),
            touched_notes: Vec::new(),
            pending_change_summary: None,
            open_questions: Vec::new(),
            last_summarized_message_id: Some("assistant-1".to_owned()),
            last_compacted_message_id: Some("assistant-1".to_owned()),
        });
        let previous_transcript = vec![
            json!({
                "role": "user",
                "content": "以下是压缩检查点，不是用户的新指令。\n\n<summary>\ncurrentGoal: 向用户介绍橘记 Agent 的能力范围\n</summary>"
            }),
            json!({
                "role": "user",
                "content": "界面 action 提示：ask\n用户输入：你能做什么"
            }),
        ];

        let prompt = build_model_prompt(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
            None,
            Some(&previous_transcript),
        );
        let joined = prompt
            .messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!joined.contains("压缩检查点"));
        assert!(joined.contains("我是橘记的本地优先知识库 Agent。"));
        assert!(prompt
            .messages
            .iter()
            .any(|message| message["role"] == "assistant"));
    }

    /** pending 活状态在本轮 user，不在 system。 */
    #[test]
    fn pending_live_state_attaches_to_current_user_not_system() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.sessions[0].pending_change = Some(runtime_test_pending_change("pending"));
        let request = runtime_test_request("ask", "继续改");
        let available_skills = crate::skills::built_in_skills();
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
            &[],
        );
        let system_content = messages[0]["content"].as_str().unwrap_or_default();
        let last_user = messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .unwrap()["content"]
            .as_str()
            .unwrap_or_default();

        assert!(!system_content.contains("【当前待确认变更】"));
        assert!(last_user.contains("【当前待确认变更】"));
        assert!(last_user.contains("尚未落盘") || last_user.contains("不要把它当成已写入文件"));
    }

    /** 全零 usage 不覆盖上次有效值。 */
    #[test]
    fn zero_usage_does_not_overwrite_previous_context_usage() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.sessions[0].context_usage = Some(AgentContextUsage {
            model_id: "test-model".to_owned(),
            prompt_tokens: 800,
            completion_tokens: 20,
            total_tokens: 820,
            recorded_at: "2026-08-26 10:00:00".to_owned(),
            context_length: Some(128_000),
        });
        record_context_usage(
            &mut snapshot.sessions[0],
            "test-model",
            &json!({ "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 } }),
            Some(128_000),
        );
        assert_eq!(
            snapshot.sessions[0]
                .context_usage
                .as_ref()
                .unwrap()
                .prompt_tokens,
            800
        );
    }

    #[test]
    fn record_context_usage_stores_model_window() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        record_context_usage(
            &mut snapshot.sessions[0],
            "gpt-4o-mini",
            &json!({ "usage": { "prompt_tokens": 1200, "completion_tokens": 80, "total_tokens": 1280 } }),
            Some(128_000),
        );
        let usage = snapshot.sessions[0].context_usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 1200);
        assert_eq!(usage.context_length, Some(128_000));
        assert_eq!(usage.model_id, "gpt-4o-mini");
    }
}
