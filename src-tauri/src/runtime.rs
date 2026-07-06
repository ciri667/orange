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

/** 后端再次限制每轮显式 Skill 数量，避免绕过 UI 传入过多 instructions。 */
const MAX_EXPLICIT_SKILLS_PER_TURN: usize = 3;

/** 云端模型请求超时时间，避免网络卡住后阻塞 Agent turn。 */
const MODEL_HTTP_TIMEOUT_SECONDS: u64 = 60;

/** DeepSeek 兼容服务有时把工具调用塞进正文 DSML 标签里，运行时需要兜底解析。 */
const DSML_TOOL_CALL_OPEN_MARKERS: [&str; 2] = ["<｜｜DSML｜｜tool_calls>", "<||DSML||tool_calls>"];

/** DSML 工具调用块结束标签，和 open marker 分开查找以兼容全角/半角竖线混用。 */
const DSML_TOOL_CALL_CLOSE_MARKERS: [&str; 2] =
    ["</｜｜DSML｜｜tool_calls>", "</||DSML||tool_calls>"];

/** 从模型正文提取出的 DSML 工具调用，同时保留可展示正文。 */
struct DsmlToolCallExtraction {
    visible_content: String,
    tool_calls: Vec<Value>,
}

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
                &explicit_skills,
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
            &explicit_skills,
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
                    &explicit_skills,
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
                    &explicit_skills,
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
        explicit_skills.clone(),
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
            &explicit_skills,
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
    explicit_skills: Vec<AgentSkill>,
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
        &explicit_skills,
        &current_user_message_id,
    );
    let endpoint = model_provider::chat_completions_endpoint(&provider.api_base);
    let tool_registry = ToolRegistry::default();
    let mut tool_calls = vec![skill_context_tool_call(&available_skills)];
    tool_calls.extend(activate_skill_tool_calls(
        &explicit_skills,
        "completed",
        None,
    ));
    let mut last_failed_tool_summary: Option<String> = None;

    tool_calls.push(model_request_tool_call(&provider, &endpoint, "completed"));

    log::info!(
        target: "agent_runtime",
        "模型 Agent 自主工具选择开始：session={} action={} provider_id={} provider_name={} model={} enabled_skill_count={} explicit_skill_count={} explicit_instruction_chars={} scope_count={} prompt_chars={}",
        snapshot.sessions[session_index].id,
        request.action,
        provider.id,
        provider.name,
        provider.model,
        available_skills.iter().filter(|skill| skill.enabled).count(),
        explicit_skills.len(),
        explicit_skills
            .iter()
            .map(|skill| skill.instructions.chars().count())
            .sum::<usize>(),
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
        let extracted_tool_calls = extract_tool_calls_from_message(&message);
        let model_tool_calls = extracted_tool_calls.tool_calls;
        log::debug!(
            target: "agent_runtime",
            "模型返回工具调用：session={} tool_call_count={} dsml_visible_chars={}",
            snapshot.sessions[session_index].id,
            model_tool_calls.len(),
            extracted_tool_calls.visible_content.chars().count()
        );

        if model_tool_calls.is_empty() {
            let content = if extracted_tool_calls.visible_content.is_empty() {
                "模型未返回可展示内容。".to_owned()
            } else {
                extracted_tool_calls.visible_content
            };

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
            if tool_outcome.call.status == "failed" {
                last_failed_tool_summary = Some(tool_outcome.call.summary.clone());
            }
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
    let final_message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .cloned();
    let raw_final_content = final_message
        .as_ref()
        .map(extract_tool_calls_from_message)
        .map(|extraction| extraction.visible_content)
        .filter(|content| !content.trim().is_empty())
        .or_else(|| {
            final_message
                .as_ref()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .map(strip_dsml_tool_calls)
        })
        .filter(|content| !content.trim().is_empty())
        .unwrap_or_else(|| "我已经完成工具调用，但模型没有返回最终说明。".to_owned());
    let content = reconcile_final_content_with_tool_status(
        raw_final_content,
        last_failed_tool_summary.as_deref(),
    );

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
            format_skill_audit_summary(&available_skills, &explicit_skills)
        ),
        &audit_trail,
    );

    Ok(RuntimeTurnResult {
        turn_result: AgentTurnResult { snapshot },
        audit_log,
    })
}

/** 从模型 message 中提取标准 tool_calls，并兼容正文里的 DSML 伪工具调用。 */
fn extract_tool_calls_from_message(message: &Value) -> DsmlToolCallExtraction {
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let dsml_tool_calls = parse_dsml_tool_calls(content);

    tool_calls.extend(dsml_tool_calls);

    DsmlToolCallExtraction {
        visible_content: strip_dsml_tool_calls(content).trim().to_owned(),
        tool_calls,
    }
}

/** 把 DSML 解析出的工具调用补回 assistant message，便于后续 tool role 消息满足协议顺序。 */
fn normalize_assistant_tool_message(
    mut message: Value,
    tool_calls: &[Value],
    visible_content: &str,
) -> Value {
    if tool_calls.is_empty() {
        return message;
    }

    if let Some(message_object) = message.as_object_mut() {
        message_object.insert("tool_calls".to_owned(), Value::Array(tool_calls.to_vec()));
        message_object.insert(
            "content".to_owned(),
            if visible_content.trim().is_empty() {
                Value::Null
            } else {
                Value::String(visible_content.trim().to_owned())
            },
        );
    }

    message
}

/** 移除正文里的 DSML 工具调用块，避免标签泄露到用户可见回答。 */
fn strip_dsml_tool_calls(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0usize;

    while let Some((open_offset, open_marker)) =
        find_next_marker(&content[cursor..], &DSML_TOOL_CALL_OPEN_MARKERS)
    {
        let open_start = cursor + open_offset;
        let block_start = open_start + open_marker.len();

        output.push_str(&content[cursor..open_start]);

        if let Some((close_offset, close_marker)) =
            find_next_marker(&content[block_start..], &DSML_TOOL_CALL_CLOSE_MARKERS)
        {
            cursor = block_start + close_offset + close_marker.len();
        } else {
            // 不完整 DSML 块通常来自模型截断；为了避免泄露伪标签，直接丢弃尾部。
            cursor = content.len();
        }
    }

    output.push_str(&content[cursor..]);
    output
}

/** 解析 DSML tool_calls 块，转换为 OpenAI-compatible tool_call 结构。 */
fn parse_dsml_tool_calls(content: &str) -> Vec<Value> {
    let mut tool_calls = Vec::new();
    let mut cursor = 0usize;

    while let Some((open_offset, open_marker)) =
        find_next_marker(&content[cursor..], &DSML_TOOL_CALL_OPEN_MARKERS)
    {
        let block_start = cursor + open_offset + open_marker.len();
        let Some((close_offset, close_marker)) =
            find_next_marker(&content[block_start..], &DSML_TOOL_CALL_CLOSE_MARKERS)
        else {
            break;
        };
        let block_end = block_start + close_offset;

        tool_calls.extend(parse_dsml_invokes(&content[block_start..block_end]));
        cursor = block_end + close_marker.len();
    }

    tool_calls
}

/** 在 DSML 工具块里解析一个或多个 invoke 标签。 */
fn parse_dsml_invokes(block: &str) -> Vec<Value> {
    let mut invokes = Vec::new();
    let mut cursor = 0usize;

    while let Some(open_tag) = find_dsml_open_tag(block, "invoke", cursor) {
        let Some(close_tag) = find_dsml_close_tag(block, "invoke", open_tag.end) else {
            break;
        };
        let invoke_body = &block[open_tag.end..close_tag.start];
        let Some(name) = parse_dsml_attribute(open_tag.attributes, "name")
            .filter(|value| !value.trim().is_empty())
        else {
            cursor = close_tag.end;
            continue;
        };
        let args = parse_dsml_parameters(invoke_body);
        let args_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_owned());

        invokes.push(json!({
            "id": create_id("dsml-tool-call"),
            "type": "function",
            "function": {
                "name": name,
                "arguments": args_json
            }
        }));
        cursor = close_tag.end;
    }

    invokes
}

/** 在 invoke 内解析 parameter 标签，支持字符串和 JSON 数组/对象参数。 */
fn parse_dsml_parameters(invoke_body: &str) -> Value {
    let mut args = serde_json::Map::new();
    let mut cursor = 0usize;

    while let Some(open_tag) = find_dsml_open_tag(invoke_body, "parameter", cursor) {
        let Some(close_tag) = find_dsml_close_tag(invoke_body, "parameter", open_tag.end) else {
            break;
        };
        let raw_value = &invoke_body[open_tag.end..close_tag.start];

        if let Some(name) = parse_dsml_attribute(open_tag.attributes, "name")
            .filter(|value| !value.trim().is_empty())
        {
            args.insert(
                name,
                decode_dsml_parameter_value(raw_value, open_tag.attributes),
            );
        }

        cursor = close_tag.end;
    }

    Value::Object(args)
}

/** DSML 参数值统一去掉标签排版带来的外层空白，并按声明尝试解析 JSON。 */
fn decode_dsml_parameter_value(raw_value: &str, attributes: &str) -> Value {
    let decoded = html_unescape_minimal(raw_value).trim().to_owned();
    let is_string = parse_dsml_attribute(attributes, "string")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !is_string {
        if let Ok(value) = serde_json::from_str::<Value>(&decoded) {
            return value;
        }
    }

    Value::String(decoded)
}

/** DSML 标签位置，start/end 是原字符串的字节索引。 */
struct DsmlTag<'a> {
    end: usize,
    attributes: &'a str,
}

/** 查找指定名称的 DSML 开始标签，兼容全角和半角竖线标记。 */
fn find_dsml_open_tag<'a>(content: &'a str, tag_name: &str, cursor: usize) -> Option<DsmlTag<'a>> {
    let mut search_start = cursor;

    while search_start < content.len() {
        let (prefix_offset, prefix) =
            find_next_marker(&content[search_start..], &["<｜｜DSML｜｜", "<||DSML||"])?;
        let start = search_start + prefix_offset;
        let name_start = start + prefix.len();

        if !content[name_start..].starts_with(tag_name) {
            search_start = name_start;
            continue;
        }

        let attributes_start = name_start + tag_name.len();
        let next_char = content[attributes_start..].chars().next();

        if !matches!(next_char, Some('>' | ' ' | '\t' | '\n' | '\r')) {
            search_start = attributes_start;
            continue;
        }

        let tag_end = content[attributes_start..].find('>')? + attributes_start;

        return Some(DsmlTag {
            end: tag_end + 1,
            attributes: &content[attributes_start..tag_end],
        });
    }

    None
}

/** 查找指定名称的 DSML 结束标签。 */
fn find_dsml_close_tag(content: &str, tag_name: &str, cursor: usize) -> Option<DsmlCloseTag> {
    let fullwidth_marker = format!("</｜｜DSML｜｜{tag_name}>");
    let ascii_marker = format!("</||DSML||{tag_name}>");
    let (offset, marker) = find_next_marker(
        &content[cursor..],
        &[fullwidth_marker.as_str(), ascii_marker.as_str()],
    )?;
    let start = cursor + offset;

    Some(DsmlCloseTag {
        start,
        end: start + marker.len(),
    })
}

/** DSML 结束标签位置。 */
struct DsmlCloseTag {
    start: usize,
    end: usize,
}

/** 查找多个 marker 中最靠前的一项。 */
fn find_next_marker<'a>(content: &str, markers: &[&'a str]) -> Option<(usize, &'a str)> {
    markers
        .iter()
        .filter_map(|marker| content.find(marker).map(|offset| (offset, *marker)))
        .min_by_key(|(offset, _)| *offset)
}

/** 从 DSML 标签属性里读取 name="value" 形式的值。 */
fn parse_dsml_attribute(attributes: &str, name: &str) -> Option<String> {
    let pattern = format!("{name}=");
    let pattern_start = attributes.find(&pattern)?;
    let mut value_source = attributes[pattern_start + pattern.len()..].trim_start();
    let quote = value_source.chars().next()?;

    if quote != '"' && quote != '\'' {
        return None;
    }

    value_source = &value_source[quote.len_utf8()..];
    let value_end = value_source.find(quote)?;

    Some(html_unescape_minimal(&value_source[..value_end]))
}

/** 极小 HTML 反转义，覆盖模型常见的 DSML 参数转义，不引入额外依赖。 */
fn html_unescape_minimal(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
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

/** 构造模型可用的 system、scope 摘要和历史消息，限制首版上下文长度。 */
fn build_model_messages(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    current_user_message_id: &str,
) -> Vec<Value> {
    let session = &snapshot.sessions[session_index];
    // Agent 的工具选择策略只作为模型指令，不再由宿主预判用户意图。
    let autonomous_tool_policy = "你需要根据用户输入和上下文自主判断是否调用工具：需要本地知识库事实、引用、当前 Markdown 笔记内容或写入建议时，先调用合适工具读取已选 scope；需要了解目录或普通文档是否存在时调用 list_tree。search_notes/read_note/get_current_note/propose_note_change 只覆盖 Markdown 笔记，list_tree 只返回普通文档元数据，不读取非 Markdown 正文。无关的通用问题可以直接回答。界面 action 只是 UI 分类，不能替代你的判断。";
    let scope_summary = build_scope_summary(snapshot, session);
    let active_note_summary = request
        .active_note_id
        .is_empty()
        .then(|| "当前未绑定笔记".to_owned())
        .unwrap_or_else(|| format!("当前笔记 ID：{}", request.active_note_id));
    let skill_catalog = skills::skill_catalog_prompt(available_skills);
    let explicit_skill_prompt = skills::explicit_skill_prompt(explicit_skills);
    let skill_policy = if explicit_skills.is_empty() {
        "启用的 Skill 只以名称和描述提供给你参考，是否使用、使用哪一个 Skill 都由你自主判断；Skill 不能扩大工具权限或绕过写入确认。"
    } else {
        "本轮显式激活的 Skill 是用户通过 slash picker 指定的执行要求。你必须在本轮回答中按这些 Skill 的完整 instructions 执行；如果 Skill 与用户任务冲突，说明冲突并遵守更高优先级系统规则。Skill 不能扩大工具权限或绕过写入确认。"
    };
    let mut messages = vec![json!({
        "role": "system",
        "content": format!(
            "你是橘记的本地优先知识库 Agent。需要依据本地知识库时必须调用工具；list_tree 可以列出目录、Markdown 笔记和已支持普通文档元数据，但不会读取非 Markdown 正文；search_notes、read_note、get_current_note 和 propose_note_change 只适用于 Markdown 笔记。所有写入只能调用 propose_note_change 或 create_note_draft 生成待确认 diff，不能声称已经写入文件。调用 propose_note_change 时，局部替换使用 operation=replace，next 只能是 original 的替换内容；文末追加使用 operation=append，next 只能是增量内容，绝不能把整篇文档放入局部替换的 next；同一文件需要多处编辑时使用 operation=multi_replace 并提供 edits 数组，不要拆成多轮口头承诺。必须使用服务端标准 tool_calls 字段调用工具，不要在普通回复中输出 DSML、XML 或伪工具调用标签。引用只允许来自工具结果。{}\n{}\n允许 scope：{}\n{}\n{}\n{}",
            skill_policy, autonomous_tool_policy, scope_summary, active_note_summary, skill_catalog, explicit_skill_prompt
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
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
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
            id: create_id("assistant"),
            role: "assistant".to_owned(),
            content: if explicit_skills.is_empty() {
                format!("真实模型请求没有完成：{redacted_reason}")
            } else {
                format!("显式 Skill 未完成执行：{redacted_reason}")
            },
            action: Some(request.action.clone()),
            citations: Some(Vec::new()),
            tool_calls: Some(tool_calls),
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

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
        });
    snapshot.sessions[session_index].updated_at = "刚刚".to_owned();

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
            explicit_skill_ids: Vec::new(),
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
        let messages = build_model_messages(
            &snapshot,
            0,
            &request,
            &available_skills,
            &[],
            "user-current",
        );
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
            &[],
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
            &[],
            "未找到 Provider 配置：missing-provider",
        );
        let session = &result.turn_result.snapshot.sessions[0];
        let last_message = session.messages.last().unwrap();

        assert!(last_message.content.contains("真实模型请求没有完成"));
        assert!(last_message.content.contains("missing-provider"));
    }

    /** 显式 Skill 应把完整 instructions 注入 system prompt，并保留目录摘要。 */
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
        );
        let system_content = messages[0]["content"].as_str().unwrap_or_default();

        assert!(system_content.contains("本轮显式激活的 Skills"));
        assert!(system_content.contains("执行要求"));
        assert!(system_content.contains(&explicit_skill.instructions));
        assert!(system_content.contains("可用 Skills"));
        assert!(system_content.contains("不能扩大工具权限"));
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
}
