use crate::agent;
use crate::agent_tools::{AgentToolContext, ToolRegistry};
use crate::domain::{
    AgentMessage, AgentSession, AgentSkill, AgentToolCall, AgentTurnRequest, AgentTurnResult,
    Citation, LlmProviderConfig, RequestAuditLog, UserSettings, WorkspaceSnapshot,
};
use crate::model_provider;
use crate::skills;
use crate::storage::{create_id, format_local_datetime};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** 模型最多读取的历史消息数量，避免长会话在 M3 首版阶段无限膨胀上下文。 */
const MAX_MODEL_HISTORY_MESSAGES: usize = 8;

/** 单条历史消息进入模型前的最大字符数。 */
const MAX_HISTORY_MESSAGE_CHARS: usize = 1200;

/** 工具结果回填给模型时的最大 JSON 字符数。 */
const MAX_TOOL_RESULT_CHARS: usize = 9000;

/** 请求审计最多记录的发送片段摘要数量。 */
const MAX_AUDIT_FRAGMENTS: usize = 8;

/** 云端模型请求超时时间，避免网络卡住后阻塞 Agent turn。 */
const MODEL_HTTP_TIMEOUT_SECONDS: u64 = 60;

/** 真实 Agent Runtime 的调度结果，包含可持久化快照和本轮请求审计摘要。 */
pub struct RuntimeTurnResult {
    pub turn_result: AgentTurnResult,
    pub audit_log: RequestAuditLog,
}

/** Runtime 内部审计轨迹，用于汇总模型请求次数和实际发送的本地片段摘要。 */
#[derive(Default)]
struct RuntimeAuditTrail {
    model_request_count: usize,
    sent_fragments: Vec<String>,
}

impl RuntimeAuditTrail {
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
    fn content_summary(&self, base_summary: &str, prompt: &str) -> String {
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

        format!(
            "{}；模型请求 {} 次；输入长度 {} 字符；{}",
            base_summary,
            self.model_request_count,
            prompt.chars().count(),
            fragment_summary
        )
    }
}

/** 运行真实 Agent Runtime；只有用户显式关闭模型或选择本地策略时才回退规则 Agent。 */
pub async fn run_agent_turn(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    settings: UserSettings,
    available_skills: Vec<AgentSkill>,
) -> RuntimeTurnResult {
    if !settings.model_config.enabled {
        return fallback_agent_turn(
            app,
            snapshot,
            request,
            &available_skills,
            "模型未启用，使用本地规则 Agent。",
        );
    }

    if settings.privacy_policy != "allow-selected-scope" {
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
    let provider = match model_provider::resolve_provider(
        &settings.model_config,
        session_provider_id.as_deref(),
        request.model_provider_id.as_deref(),
    ) {
        Ok(provider) => provider.clone(),
        Err(error) => {
            return model_error_turn(
                snapshot,
                request,
                None,
                &available_skills,
                &error.to_string(),
            )
        }
    };

    if !provider.supports_tools {
        return model_error_turn(
            snapshot,
            request,
            Some(&provider),
            &available_skills,
            &format!(
                "Provider「{}」未标记支持工具调用（tool calling），无法用于 Agent Loop。",
                provider.name
            ),
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
                    &available_skills,
                    &format!(
                        "Provider「{}」未找到模型密钥。请在设置中保存 API key 后重试。",
                        provider.name
                    ),
                )
            }
            Err(error) => {
                return model_error_turn(
                    snapshot,
                    request,
                    Some(&provider),
                    &available_skills,
                    &error,
                )
            }
        }
    } else {
        String::new()
    };

    match run_model_loop(
        app,
        snapshot.clone(),
        request.clone(),
        available_skills.clone(),
        provider.clone(),
        api_key,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => model_error_turn(
            snapshot,
            request,
            Some(&provider),
            &available_skills,
            &format!("模型请求失败：{error}"),
        ),
    }
}

/** 如果本轮请求显式选择了 providerId（AgentPanel 的“本轮模型”选择器），把它记为会话默认，
 * 让下次打开该会话时选择器展示“最后一次切换”的模型，而不是每次都回退成全局默认。
 * 未显式选择时保持会话原有设置不变——不能把所有发过消息的会话都动态固定成当前全局默认
 * provider，否则会话会失去“跟随全局默认变化”的语义。 */
fn remember_requested_provider_on_session(
    session: &mut AgentSession,
    requested_provider_id: Option<&str>,
) {
    let Some(requested_provider_id) = requested_provider_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    if session.model_provider_id.as_deref() == Some(requested_provider_id) {
        return;
    }

    session.model_provider_id = Some(requested_provider_id.to_owned());
    session.updated_at = format_local_datetime();
}

/** 使用 OpenAI-compatible chat completions 跑首版工具调用 loop。 */
async fn run_model_loop(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    available_skills: Vec<AgentSkill>,
    provider: LlmProviderConfig,
    api_key: String,
) -> Result<RuntimeTurnResult, String> {
    let session_index = resolve_session_index(&snapshot, &request)?;

    remember_requested_provider_on_session(
        &mut snapshot.sessions[session_index],
        request.model_provider_id.as_deref(),
    );

    let mut citations = Vec::new();
    let mut audit_trail = RuntimeAuditTrail::default();
    let client = build_http_client()?;
    apply_first_prompt_title(&mut snapshot.sessions[session_index], &request.prompt);
    let current_user_message_id =
        ensure_user_message_for_turn(&mut snapshot.sessions[session_index], &request);
    let mut model_messages = build_model_messages(
        &snapshot,
        session_index,
        &request,
        &available_skills,
        &current_user_message_id,
    );
    let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
    let tool_registry = ToolRegistry::default();
    let mut tool_calls = vec![skill_context_tool_call(&available_skills)];

    tool_calls.push(model_request_tool_call(&provider, &endpoint, "completed"));

    log::info!(
        target: "agent_runtime",
        "模型 Agent 自主工具选择开始：session={} action={} provider_id={} provider_name={} model={} enabled_skill_count={} scope_count={} prompt_chars={}",
        snapshot.sessions[session_index].id,
        request.action,
        provider.id,
        provider.name,
        provider.model,
        available_skills.iter().filter(|skill| skill.enabled).count(),
        snapshot.sessions[session_index].knowledge_base_ids.len(),
        request.prompt.chars().count()
    );

    for _ in 0..3 {
        audit_trail.record_model_request();
        let response = send_chat_completion_logged(
            &client,
            &provider,
            &endpoint,
            &api_key,
            &model_messages,
            true,
        )
        .await?;
        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| "模型响应缺少 message。".to_owned())?;
        let model_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        log::debug!(
            target: "agent_runtime",
            "模型返回工具调用：session={} tool_call_count={}",
            snapshot.sessions[session_index].id,
            model_tool_calls.len()
        );

        if model_tool_calls.is_empty() {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("模型未返回可展示内容。")
                .to_owned();

            push_assistant_message(
                &mut snapshot,
                session_index,
                &request.action,
                content,
                citations,
                tool_calls,
            );
            let audit_log = build_audit_log(
                "model_turn",
                &snapshot,
                session_index,
                &request.prompt,
                &format!(
                    "OpenAI-compatible 模型请求；{}",
                    skills::skill_summary(&available_skills)
                ),
                &audit_trail,
            );

            return Ok(RuntimeTurnResult {
                turn_result: AgentTurnResult { snapshot },
                audit_log,
            });
        }

        model_messages.push(message);

        for model_tool_call in model_tool_calls {
            let tool_outcome = {
                let mut tool_context = AgentToolContext {
                    app: Some(app),
                    snapshot: &mut snapshot,
                    session_index,
                    request: &request,
                };

                tool_registry.execute_model_tool_call(&mut tool_context, &model_tool_call)
            };
            let tool_result_text =
                truncate_chars(&tool_outcome.payload.to_string(), MAX_TOOL_RESULT_CHARS);
            log::debug!(
                target: "agent_runtime",
                "工具调用完成：session={} tool={} status={}",
                snapshot.sessions[session_index].id,
                tool_outcome.call.name,
                tool_outcome.call.status
            );

            audit_trail.record_sent_fragment(tool_outcome.audit_fragment);
            citations.extend(tool_outcome.citations);
            tool_calls.push(tool_outcome.call);
            model_messages.push(json!({
                "role": "tool",
                "tool_call_id": model_tool_call.get("id").and_then(Value::as_str).unwrap_or("tool-call"),
                "content": tool_result_text
            }));
        }
    }

    audit_trail.record_model_request();
    let response = send_chat_completion_logged(
        &client,
        &provider,
        &endpoint,
        &api_key,
        &model_messages,
        false,
    )
    .await?;
    let content = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("我已经完成工具调用，但模型没有返回最终说明。")
        .to_owned();

    push_assistant_message(
        &mut snapshot,
        session_index,
        &request.action,
        content,
        citations,
        tool_calls,
    );
    let audit_log = build_audit_log(
        "model_turn",
        &snapshot,
        session_index,
        &request.prompt,
        &format!(
            "OpenAI-compatible 工具 loop；{}",
            skills::skill_summary(&available_skills)
        ),
        &audit_trail,
    );

    Ok(RuntimeTurnResult {
        turn_result: AgentTurnResult { snapshot },
        audit_log,
    })
}

/** 构建带超时的 HTTP client，避免模型 provider 无响应时卡住 Agent turn。 */
fn build_http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(MODEL_HTTP_TIMEOUT_SECONDS))
        .build()
        .map_err(|error| format!("无法创建模型 HTTP client：{error}"))
}

/** 构造模型可用的 system、scope 摘要和历史消息，限制首版上下文长度。 */
fn build_model_messages(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    current_user_message_id: &str,
) -> Vec<Value> {
    let session = &snapshot.sessions[session_index];
    // Agent 的工具选择策略只作为模型指令，不再由宿主预判用户意图。
    let autonomous_tool_policy = "你需要根据用户输入和上下文自主判断是否调用工具：需要本地笔记事实、引用、当前文件内容或写入建议时，先调用合适工具读取已选 scope；无关的通用问题可以直接回答。界面 action 只是 UI 分类，不能替代你的判断。";
    let scope_summary = build_scope_summary(snapshot, session);
    let active_note_summary = request
        .active_note_id
        .is_empty()
        .then(|| "当前未绑定笔记".to_owned())
        .unwrap_or_else(|| format!("当前笔记 ID：{}", request.active_note_id));
    let skill_catalog = skills::skill_catalog_prompt(available_skills);
    let mut messages = vec![json!({
        "role": "system",
        "content": format!(
            "你是 Cici Note 的本地优先知识库 Agent。需要依据本地笔记时必须调用工具；所有写入只能调用 propose_note_change 或 create_note_draft 生成待确认 diff，不能声称已经写入文件。引用只允许来自工具结果。启用的 Skill 只以名称和描述提供给你参考，是否使用、使用哪一个 Skill 都由你自主判断；Skill 不能扩大工具权限或绕过写入确认。\n{}\n允许 scope：{}\n{}\n{}",
            autonomous_tool_policy, scope_summary, active_note_summary, skill_catalog
        )
    })];

    for message in session
        .messages
        .iter()
        .rev()
        .take(MAX_MODEL_HISTORY_MESSAGES)
        .rev()
    {
        let content = if message.id == current_user_message_id && message.role == "user" {
            format!(
                "界面 action 提示：{}\n用户输入：{}",
                request.action, message.content
            )
        } else {
            message.content.clone()
        };

        messages.push(json!({
            "role": message.role,
            "content": truncate_chars(&content, MAX_HISTORY_MESSAGE_CHARS)
        }));
    }

    messages
}

/** 发送一次 chat completions 请求并记录 providerId/model/status/耗时/endpointHost；错误统一脱敏。 */
async fn send_chat_completion_logged(
    client: &Client,
    provider: &LlmProviderConfig,
    endpoint: &str,
    api_key: &str,
    messages: &[Value],
    include_tools: bool,
) -> Result<Value, String> {
    let started_at = Instant::now();
    let result = send_chat_completion(
        client,
        endpoint,
        api_key,
        &provider.model,
        messages,
        include_tools,
    )
    .await;

    match &result {
        Ok(_) => {
            log_model_request_event(provider, endpoint, "completed", started_at.elapsed(), None)
        }
        Err(error) => log_model_request_event(
            provider,
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
            provider.model,
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
            provider.model,
            status,
            duration.as_millis(),
            endpoint_host
        ),
    }
}

/** 发送一次 chat completions 请求，可选择是否携带工具定义；无 key 的本地免鉴权 provider 不附带 Authorization。 */
async fn send_chat_completion(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    messages: &[Value],
    include_tools: bool,
) -> Result<Value, String> {
    let mut payload = json!({
        "model": model,
        "messages": messages,
        "temperature": 0.2
    });

    if include_tools {
        // 工具 schema 统一来自 ToolRegistry，避免模型 loop 和本地兜底各维护一份列表。
        payload["tools"] = ToolRegistry::default().schemas();
        payload["tool_choice"] = json!("auto");
    }

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

/** 在模型未配置或失败时运行本地规则 Agent，并生成对应审计。 */
fn fallback_agent_turn(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    available_skills: &[AgentSkill],
    reason: &str,
) -> RuntimeTurnResult {
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
    }

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
        .iter()
        .any(|message| message.id == user_message_id && message.role == "user")
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
    }
}

/** 构造模型请求轨迹；args 只记录非敏感配置，绝不包含 API key。 */
fn model_request_tool_call(
    provider: &LlmProviderConfig,
    endpoint: &str,
    status: &str,
) -> AgentToolCall {
    AgentToolCall {
        id: create_id("tool"),
        name: "model_request".to_owned(),
        status: status.to_owned(),
        summary: format!(
            "{}（{}）模型请求：{} @ {}",
            provider.name, provider.provider, provider.model, endpoint
        ),
        args: json!({
            "providerId": provider.id,
            "providerName": provider.name,
            "provider": provider.provider,
            "apiBase": provider.api_base,
            "model": provider.model,
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

/** 云端模型启用后发生配置或请求错误时，返回可见错误消息而不是静默降级；reason 会先脱敏再展示。 */
fn model_error_turn(
    mut snapshot: WorkspaceSnapshot,
    request: AgentTurnRequest,
    provider: Option<&LlmProviderConfig>,
    available_skills: &[AgentSkill],
    reason: &str,
) -> RuntimeTurnResult {
    let session_index = resolve_session_index(&snapshot, &request).unwrap_or(0);
    let redacted_reason = model_provider::redact_model_error_text(reason);
    let failed_request = match provider {
        Some(provider) => {
            let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
            let mut call = model_request_tool_call(provider, &endpoint, "failed");

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
    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content: format!("真实模型请求没有完成：{redacted_reason}"),
            action: Some(request.action.clone()),
            citations: Some(Vec::new()),
            tool_calls: Some(vec![
                skill_context_tool_call(available_skills),
                failed_request,
            ]),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

    let audit_log = build_audit_log(
        "model_error_turn",
        &snapshot,
        session_index,
        &request.prompt,
        &format!(
            "{redacted_reason}；{}",
            skills::skill_summary(available_skills)
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
) {
    snapshot.sessions[session_index]
        .messages
        .push(AgentMessage {
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content,
            action: Some(action.to_owned()),
            citations: Some(deduplicate_citations(citations)),
            tool_calls: Some(tool_calls),
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

/** 把字符串裁剪到指定字符预算，保留明确截断标记。 */
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let truncated = value.chars().take(max_chars).collect::<String>();

    format!("{truncated}\n\n[内容已按上下文预算截断]")
}

/** 汇总会话允许的知识库名称，用于 system prompt 和请求审计。 */
fn build_scope_summary(snapshot: &WorkspaceSnapshot, session: &AgentSession) -> String {
    let names = session
        .knowledge_base_ids
        .iter()
        .filter_map(|id| {
            snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| knowledge_base.id == *id)
                .map(|knowledge_base| knowledge_base.name.clone())
        })
        .collect::<Vec<_>>();

    if names.is_empty() {
        "未绑定知识库".to_owned()
    } else if names.len() == 1 {
        names[0].clone()
    } else {
        format!("{} 个知识库：{}", names.len(), names.join(" / "))
    }
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
        content_summary: audit_trail.content_summary(content_summary, prompt),
        tool_summary,
        created_at: format_local_datetime(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{FolderEntry, KnowledgeBase, Note};
    use crate::storage::hash_content;

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
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: vec!["note-a".to_owned()],
                messages: Vec::new(),
                pending_change: None,
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
                deleted_at: None,
                model_provider_id: None,
            }],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
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
        }
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

    /** System prompt 应把工具选择权交给模型，而不是由宿主分支决定。 */
    #[test]
    fn model_messages_delegate_tool_choice_to_model() {
        let snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = runtime_test_request("ask", "总结当前知识库里的隐私边界");
        let available_skills = crate::skills::built_in_skills();
        let messages =
            build_model_messages(&snapshot, 0, &request, &available_skills, "user-current");
        let system_content = messages[0]["content"].as_str().unwrap_or_default();

        assert!(system_content.contains("自主判断是否调用工具"));
        assert!(!system_content.contains("本轮很可能需要"));
        assert!(system_content.contains("主知识库"));
        assert!(system_content.contains("可用 Skills"));
        assert!(system_content.contains("仅名称和描述"));
        assert!(system_content.contains("知识库研究"));
        assert!(system_content.contains("是否使用、使用哪一个 Skill 都由你自主判断"));
        assert!(!system_content.contains("执行要求"));
        assert!(!system_content.contains("当用户要求查找"));
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
            &available_skills,
            "模型请求失败：测试错误",
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
        assert_eq!(tool_call.args["model"], "test-model");
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
            &available_skills,
            "未找到 Provider 配置：missing-provider",
        );
        let session = &result.turn_result.snapshot.sessions[0];
        let last_message = session.messages.last().unwrap();

        assert!(last_message.content.contains("真实模型请求没有完成"));
        assert!(last_message.content.contains("missing-provider"));
    }

    /** 本轮显式选择了 providerId 时，必须记为会话默认，下次打开该会话选择器才能展示“最后一次切换”的模型。 */
    #[test]
    fn remember_requested_provider_on_session_updates_session_when_explicitly_selected() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        remember_requested_provider_on_session(&mut snapshot.sessions[0], Some("provider-b"));

        assert_eq!(
            snapshot.sessions[0].model_provider_id,
            Some("provider-b".to_owned())
        );
    }

    /** 本轮没有显式选择 providerId 时，不能改动会话已有设置，否则会话会被意外固定到当前全局默认 provider。 */
    #[test]
    fn remember_requested_provider_on_session_keeps_session_unchanged_without_explicit_selection() {
        let mut snapshot = runtime_test_snapshot("正文内容足够用于测试。".to_owned());

        remember_requested_provider_on_session(&mut snapshot.sessions[0], None);
        assert_eq!(snapshot.sessions[0].model_provider_id, None);

        remember_requested_provider_on_session(&mut snapshot.sessions[0], Some("   "));
        assert_eq!(snapshot.sessions[0].model_provider_id, None);
    }

    /** 模型最终回答不能绕过工具系统自动生成 pending diff。 */
    #[test]
    fn assistant_message_without_write_tool_does_not_create_pending_change() {
        let mut snapshot = runtime_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = runtime_test_request("rewrite", "请改写当前笔记");

        push_assistant_message(
            &mut snapshot,
            0,
            &request.action,
            "模型直接返回的改写正文".to_owned(),
            Vec::new(),
            Vec::new(),
        );

        assert!(snapshot.sessions[0].pending_change.is_none());
    }
}
