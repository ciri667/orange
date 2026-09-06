//! 进程内子 Agent：全新或续跑上下文、只读/写作角色、并行与后台结算。

use crate::agent_tools::{
    model_tool_call_name, parse_tool_args, AgentToolContext, ToolOutcome, ToolRegistry,
};
use crate::agent_trace::AgentTurnTracer;
use crate::domain::{
    AgentToolCall, AgentTurnRequest, Citation, LlmProviderConfig, ProposedChange,
    ProposedChangeSet, ProposedFileOperation, WorkspaceSnapshot, AGENT_DIRECT_EXECUTION_ID,
    AGENT_DIRECT_SOURCE,
};
use crate::model_provider;
use crate::provider_error;
use crate::storage::{self, create_id, format_local_datetime};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::AppHandle;

use super::cancel::{is_user_abort_error, AgentCancel};
use super::context::truncate_chars;
use super::dsml::{extract_tool_calls_from_message, normalize_assistant_tool_message};
use super::stream::stream_ui_progress;
use super::subagent_personas::{
    build_subagent_system_prompt, catalog_names, resolve_agent, ResolvedAgent,
};
use super::{send_chat_completion_with_policy, MAX_INNER_TOOL_ROUNDS, MAX_TOOL_RESULT_CHARS};

/** 顶层可委派，子级深度为 1 时禁止再委派。 */
pub(super) const MAX_SUBAGENT_DEPTH: u32 = 1;

/** 同一条 assistant 消息里最多同时跑多少个只读子 Agent。 */
pub(super) const MAX_PARALLEL_SUBAGENTS: usize = 3;

/** 交回父模型的终稿字符帽，完整轨迹留在过程区 children。 */
const MAX_SUBAGENT_RESULT_CHARS: usize = 6000;

const META_SUFFIX: &str = ":meta";
const INBOX_PREFIX: &str = "subagent-inbox:";

/** 父级 runtime 派发 task 时传入的嵌套循环参数。 */
pub(super) struct SubagentRunParams<'a> {
    pub app: &'a AppHandle,
    pub snapshot: &'a mut WorkspaceSnapshot,
    pub session_index: usize,
    pub request: &'a AgentTurnRequest,
    pub args: &'a Value,
    pub parent_depth: u32,
    pub cancel: &'a AgentCancel,
    pub tracer: SubagentTracer,
    pub parent_trace_step_id: Option<String>,
    pub provider: &'a LlmProviderConfig,
    pub selected_model_id: &'a str,
    pub api_key: &'a str,
    pub client: &'a Client,
    pub emit_progress: bool,
}

/** 过程区写入走互斥，以便并行和后台任务跨 await 仍是 Send。 */
#[derive(Clone)]
pub(super) struct SubagentTracer(Arc<Mutex<AgentTurnTracer>>);

impl SubagentTracer {
    pub(super) fn shared(tracer: Arc<Mutex<AgentTurnTracer>>) -> Self {
        Self(tracer)
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut AgentTurnTracer) -> R) -> R {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }
}

#[derive(Clone, Debug)]
struct ParsedTask {
    description: String,
    prompt: String,
    agent: ResolvedAgent,
    task_id: String,
    resume: bool,
    background: bool,
}

/** 只读、前台、非续跑写作的 task 可以和同一批兄弟并行。 */
pub(super) fn is_parallelizable_task(args: &Value) -> bool {
    let Ok(parsed) = parse_task_args(args) else {
        return false;
    };
    !parsed.background && !parsed.agent.writable()
}

pub(super) fn parallel_window_size(args_list: &[Value]) -> usize {
    let mut count = 0usize;
    for args in args_list {
        if !is_parallelizable_task(args) {
            break;
        }
        count += 1;
        if count >= MAX_PARALLEL_SUBAGENTS {
            break;
        }
    }
    count
}

/** 启动一次性或续跑子 Agent，失败以工具结果返回，不打爆父级回合。 */
pub(super) async fn run_one_shot(mut params: SubagentRunParams<'_>) -> ToolOutcome {
    let parsed = match parse_task_args(params.args) {
        Ok(parsed) => parsed,
        Err(message) => return failed_outcome(params.args, &message, None),
    };

    if params.parent_depth >= MAX_SUBAGENT_DEPTH {
        return failed_outcome(
            params.args,
            &format!("已达到子 Agent 深度上限（{MAX_SUBAGENT_DEPTH}）。子 Agent 不能再委派 task。"),
            Some(&parsed.task_id),
        );
    }

    if parsed.background && parsed.agent.writable() {
        return failed_outcome(
            params.args,
            "writer 以及带 edit/write 的自定义角色不能在后台运行，以免和父级抢同一份待确认写入。",
            Some(&parsed.task_id),
        );
    }

    if params.cancel.is_aborted() {
        return aborted_outcome(params.args, Some(&parsed.task_id));
    }

    let running_summary = format!("正在委派 {}：{}", parsed.agent.label, parsed.description);
    let parent_step = params.parent_trace_step_id.clone();
    params.tracer.with_mut(|tracer| {
        tracer.annotate_tool(
            parent_step.as_deref(),
            Some(&parsed.agent.name),
            &running_summary,
            Some(params.app),
        );
        tracer.set_step_task_id(parent_step.as_deref(), &parsed.task_id, Some(params.app));
    });

    if parsed.background {
        return spawn_background(&params, parsed);
    }

    match run_nested_loop(&mut params, &parsed).await {
        Ok(result) => result,
        Err(error) if params.cancel.is_aborted() || is_user_abort_error(&error) => {
            aborted_outcome(params.args, Some(&parsed.task_id))
        }
        Err(error) => failed_outcome(
            params.args,
            &provider_error::user_facing_provider_error(&error),
            Some(&parsed.task_id),
        ),
    }
}

fn spawn_background(params: &SubagentRunParams<'_>, parsed: ParsedTask) -> ToolOutcome {
    let app = params.app.clone();
    let snapshot = params.snapshot.clone();
    let request = params.request.clone();
    let args = params.args.clone();
    let cancel = params.cancel.clone();
    let provider = params.provider.clone();
    let selected_model_id = params.selected_model_id.to_owned();
    let api_key = params.api_key.to_owned();
    let client = params.client.clone();
    let tracer = params.tracer.clone();
    let emit_progress = params.emit_progress;
    let parent_trace_step_id = params.parent_trace_step_id.clone();
    let session_index = params.session_index;
    let parent_depth = params.parent_depth;
    let parent_session_id = params.snapshot.sessions[params.session_index].id.clone();
    let task_id = parsed.task_id.clone();
    let description = parsed.description.clone();
    let agent_name = parsed.agent.name.clone();

    let parsed_for_result = parsed.clone();
    tauri::async_runtime::spawn(async move {
        let mut snapshot = snapshot;
        let mut job_params = SubagentRunParams {
            app: &app,
            snapshot: &mut snapshot,
            session_index,
            request: &request,
            args: &args,
            parent_depth,
            cancel: &cancel,
            tracer,
            parent_trace_step_id,
            provider: &provider,
            selected_model_id: &selected_model_id,
            api_key: &api_key,
            client: &client,
            emit_progress,
        };
        let outcome = match run_nested_loop(&mut job_params, &parsed).await {
            Ok(result) => result,
            Err(error) if cancel.is_aborted() || is_user_abort_error(&error) => {
                aborted_outcome(&args, Some(&parsed.task_id))
            }
            Err(error) => failed_outcome(
                &args,
                &provider_error::user_facing_provider_error(&error),
                Some(&parsed.task_id),
            ),
        };
        let notice = format_background_notice(&task_id, &agent_name, &description, &outcome);
        push_notice(&app, &parent_session_id, notice);
    });

    running_outcome(params.args, &parsed_for_result)
}

/** 并行跑一组只读前台 task；调用方负责把 tracer 放进互斥并在 join 后取回。 */
pub(super) async fn run_parallel_readonly(
    app: AppHandle,
    snapshot: WorkspaceSnapshot,
    session_index: usize,
    request: AgentTurnRequest,
    jobs: Vec<(Value, Option<String>)>,
    parent_depth: u32,
    cancel: AgentCancel,
    tracer: Arc<Mutex<AgentTurnTracer>>,
    provider: LlmProviderConfig,
    selected_model_id: String,
    api_key: String,
    client: Client,
) -> Vec<ToolOutcome> {
    let mut handles = Vec::new();
    for (tool_call, step_id) in jobs {
        let args = parse_tool_args(&tool_call);
        let app = app.clone();
        let mut snapshot = snapshot.clone();
        let request = request.clone();
        let cancel = cancel.clone();
        let tracer = tracer.clone();
        let provider = provider.clone();
        let selected_model_id = selected_model_id.clone();
        let api_key = api_key.clone();
        let client = client.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            run_one_shot(SubagentRunParams {
                app: &app,
                snapshot: &mut snapshot,
                session_index,
                request: &request,
                args: &args,
                parent_depth,
                cancel: &cancel,
                tracer: SubagentTracer::shared(tracer),
                parent_trace_step_id: step_id,
                provider: &provider,
                selected_model_id: &selected_model_id,
                api_key: &api_key,
                client: &client,
                emit_progress: true,
            })
            .await
        }));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(outcome) => outcomes.push(outcome),
            Err(error) => outcomes.push(failed_outcome(
                &json!({}),
                &format!("并行子 Agent 失败：{error}"),
                None,
            )),
        }
    }
    outcomes
}

async fn run_nested_loop(
    params: &mut SubagentRunParams<'_>,
    parsed: &ParsedTask,
) -> Result<ToolOutcome, String> {
    let mut model_messages = if parsed.resume {
        load_child_messages(params.app, &parsed.task_id)?.ok_or_else(|| {
            format!(
                "找不到可续跑的子 Agent：{}。请去掉 task_id 重新委派。",
                parsed.task_id
            )
        })?
    } else {
        let system =
            build_subagent_system_prompt(params.snapshot, params.session_index, &parsed.agent);
        vec![
            json!({ "role": "system", "content": system }),
            json!({ "role": "user", "content": parsed.prompt }),
        ]
    };
    if parsed.resume {
        model_messages.push(json!({ "role": "user", "content": parsed.prompt }));
    }

    let registry = ToolRegistry::for_subagent_tools(&parsed.agent.tools);
    let tool_schemas = registry.schemas();
    let endpoint = model_provider::chat_completions_endpoint(&params.provider.api_base);
    let mut citations: Vec<Citation> = Vec::new();
    let mut last_failed: Option<String> = None;
    let started = std::time::Instant::now();
    let mut nested_steps: u32 = 0;
    let parent_step = params.parent_trace_step_id.clone();
    let trace_app = params.emit_progress.then_some(params.app);

    if parsed.agent.writable() {
        fold_pending_change_into_change_set(&mut params.snapshot.sessions[params.session_index]);
    }

    for _ in 0..MAX_INNER_TOOL_ROUNDS {
        if params.cancel.is_aborted() {
            persist_child(params.app, parsed, &model_messages);
            return Ok(aborted_outcome(params.args, Some(&parsed.task_id)));
        }

        let response = send_chat_completion_with_policy(
            params.client,
            params.provider,
            params.selected_model_id,
            &endpoint,
            params.api_key,
            &mut model_messages,
            Some(&tool_schemas),
            None,
            params.cancel,
            &mut |streamed| {
                let progress = stream_ui_progress(streamed);
                let trace_app = params.emit_progress.then_some(params.app);
                params.tracer.with_mut(|tracer| {
                    if !progress.thinking.is_empty() {
                        tracer.update_nested_thinking(
                            parent_step.as_deref(),
                            &progress.thinking,
                            trace_app,
                        );
                    }
                    if !progress.content.is_empty() {
                        tracer.set_nested_result_preview(
                            parent_step.as_deref(),
                            &progress.content,
                            trace_app,
                        );
                    }
                });
            },
        )
        .await?;

        let finish_reason = provider_error::parse_finish_reason(&response);
        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| "子 Agent 模型响应缺少 message。".to_owned())?;
        let extracted = extract_tool_calls_from_message(&message);
        let model_tool_calls = extracted.tool_calls;

        if provider_error::is_length_stop(finish_reason.as_deref()) && !model_tool_calls.is_empty()
        {
            let failure_text = "子 Agent 工具参数可能被截断，本批调用未执行。";
            model_messages.push(normalize_assistant_tool_message(
                message,
                &model_tool_calls,
                &extracted.visible_content,
            ));
            for model_tool_call in &model_tool_calls {
                model_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": model_tool_call.get("id").and_then(Value::as_str).unwrap_or("tool-call"),
                    "content": json!({ "success": false, "error": failure_text }).to_string()
                }));
            }
            last_failed = Some(failure_text.to_owned());
            continue;
        }

        if model_tool_calls.is_empty() {
            let content = if extracted.visible_content.is_empty() {
                if provider_error::is_length_stop(finish_reason.as_deref()) {
                    "子 Agent 输出因长度限制被截断，没有完整回复。".to_owned()
                } else {
                    "子 Agent 未返回可展示内容。".to_owned()
                }
            } else {
                extracted.visible_content
            };
            if parsed.agent.writable() {
                fold_pending_change_into_change_set(
                    &mut params.snapshot.sessions[params.session_index],
                );
            }
            persist_child(params.app, parsed, &model_messages);
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            return Ok(completed_outcome(
                params.args,
                parsed,
                &content,
                nested_steps,
                duration_ms,
                citations,
            ));
        }

        model_messages.push(normalize_assistant_tool_message(
            message,
            &model_tool_calls,
            &extracted.visible_content,
        ));
        let should_push_thinking = params.tracer.with_mut(|tracer| {
            !tracer.last_nested_step_is_thinking(parent_step.as_deref())
                && !extracted.visible_content.trim().is_empty()
        });
        if should_push_thinking {
            params.tracer.with_mut(|tracer| {
                tracer.update_nested_thinking(
                    parent_step.as_deref(),
                    &extracted.visible_content,
                    trace_app,
                );
            });
        }

        for model_tool_call in model_tool_calls {
            if params.cancel.is_aborted() {
                persist_child(params.app, parsed, &model_messages);
                return Ok(aborted_outcome(params.args, Some(&parsed.task_id)));
            }
            let tool_name = model_tool_call_name(&model_tool_call);
            let tool_args = parse_tool_args(&model_tool_call);
            let child_step_id = params.tracer.with_mut(|tracer| {
                tracer.begin_nested_tool(
                    parent_step.as_deref(),
                    &tool_name,
                    &format!("正在调用 {tool_name}"),
                    tool_args,
                    trace_app,
                )
            });
            let tool_outcome = {
                let mut tool_context = AgentToolContext {
                    app: Some(params.app),
                    snapshot: params.snapshot,
                    session_index: params.session_index,
                    request: params.request,
                };
                registry.execute_model_tool_call(&mut tool_context, &model_tool_call)
            };
            nested_steps = nested_steps.saturating_add(1);
            let tool_result_text = super::context::truncate_tool_result_for_model(
                &tool_outcome.payload.to_string(),
                MAX_TOOL_RESULT_CHARS,
            );
            let tool_error = if tool_outcome.call.status == "failed" {
                Some(tool_outcome.call.summary.clone())
            } else {
                None
            };
            params.tracer.with_mut(|tracer| {
                tracer.finish_tool(
                    child_step_id.as_deref(),
                    &tool_outcome.call.status,
                    &tool_outcome.call.summary,
                    Some(&tool_result_text),
                    tool_error.as_deref(),
                    trace_app,
                );
            });
            if let Some(tool_error) = tool_error {
                last_failed = Some(tool_error);
            }
            citations.extend(tool_outcome.citations);
            model_messages.push(json!({
                "role": "tool",
                "tool_call_id": model_tool_call.get("id").and_then(Value::as_str).unwrap_or("tool-call"),
                "content": tool_result_text
            }));
        }
    }

    if parsed.agent.writable() {
        fold_pending_change_into_change_set(&mut params.snapshot.sessions[params.session_index]);
    }
    persist_child(params.app, parsed, &model_messages);
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let content = last_failed
        .map(|failed| format!("子 Agent 已达到工具步数上限。最后一次失败：{failed}"))
        .unwrap_or_else(|| "子 Agent 已达到工具步数上限，请根据已有结果继续。".to_owned());
    Ok(completed_outcome(
        params.args,
        parsed,
        &content,
        nested_steps,
        duration_ms,
        citations,
    ))
}

fn parse_task_args(args: &Value) -> Result<ParsedTask, String> {
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "task 需要非空 description。".to_owned())?
        .to_owned();
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "task 需要非空 prompt，子 Agent 看不到当前对话。".to_owned())?
        .to_owned();
    let task_id = args
        .get("task_id")
        .or_else(|| args.get("taskId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let resume = task_id.is_some();
    let task_id = task_id.unwrap_or_else(|| create_id("subagent"));
    let agent_name = args
        .get("agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let agent = if let Some(name) = agent_name {
        resolve_agent(name)?
    } else if resume {
        resolve_agent("explore")?
    } else {
        return Err(format!("task 需要 agent。可用：{}。", catalog_names()));
    };
    let background = args
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ParsedTask {
        description,
        prompt,
        agent,
        task_id,
        resume,
        background,
    })
}

fn persist_child(app: &AppHandle, parsed: &ParsedTask, messages: &[Value]) {
    if let Err(error) = storage::save_agent_session_transcript(app, &parsed.task_id, messages) {
        log::warn!(
            target: "agent_runtime",
            "保存子 Agent transcript 失败：task_id={} error={}",
            parsed.task_id,
            error
        );
    }
    let meta = json!([{
        "agent": parsed.agent.name,
        "description": parsed.description,
    }]);
    if let Err(error) = storage::save_agent_session_transcript(
        app,
        &format!("{}{META_SUFFIX}", parsed.task_id),
        &[meta],
    ) {
        log::warn!(
            target: "agent_runtime",
            "保存子 Agent 元数据失败：task_id={} error={}",
            parsed.task_id,
            error
        );
    }
}

fn load_child_messages(app: &AppHandle, task_id: &str) -> Result<Option<Vec<Value>>, String> {
    storage::load_agent_session_transcript(app, task_id)
}

/** 把单槽 pending_change 折进 agent-direct 变更集，让写作子 Agent 不再和父级抢槽。 */
pub(super) fn fold_pending_change_into_change_set(session: &mut crate::domain::AgentSession) {
    let Some(change) = session.pending_change.take() else {
        return;
    };
    if change.status != "pending" {
        session.pending_change = Some(change);
        return;
    }
    if let Some(existing) = session.pending_change_set.as_ref() {
        if existing.status == "pending" && existing.execution_id != AGENT_DIRECT_EXECUTION_ID {
            session.pending_change = Some(change);
            return;
        }
    }
    let operation = pending_change_to_operation(&change);
    let mut change_set = session
        .pending_change_set
        .take()
        .filter(|set| set.execution_id == AGENT_DIRECT_EXECUTION_ID)
        .unwrap_or_else(|| ProposedChangeSet {
            id: create_id("change-set"),
            execution_id: AGENT_DIRECT_EXECUTION_ID.to_owned(),
            skill_id: AGENT_DIRECT_SOURCE.to_owned(),
            status: "pending".to_owned(),
            summary: String::new(),
            operations: Vec::new(),
            warnings: Vec::new(),
            created_at: format_local_datetime(),
        });
    change_set.operations.push(operation);
    change_set.summary = change.title.clone();
    change_set.status = "pending".to_owned();
    change_set.created_at = format_local_datetime();
    session.pending_change_set = Some(change_set);
    session.updated_at = format_local_datetime();
}

fn pending_change_to_operation(change: &ProposedChange) -> ProposedFileOperation {
    let operation = if change.r#type == "create" {
        "create"
    } else {
        "modify"
    };
    ProposedFileOperation {
        id: change.id.clone(),
        knowledge_base_id: change.knowledge_base_id.clone(),
        operation: operation.to_owned(),
        source_path: None,
        target_path: change.target_path.clone(),
        file_type: change
            .file_type
            .clone()
            .unwrap_or_else(|| "markdown".to_owned()),
        original_hash: change.original_hash.clone(),
        original: Some(change.original.clone()),
        next: Some(change.next.clone()),
        selected: true,
        binary: false,
        byte_size: change.next.len(),
        staged_path: None,
    }
}

fn completed_outcome(
    args: &Value,
    parsed: &ParsedTask,
    content: &str,
    nested_steps: u32,
    duration_ms: u64,
    citations: Vec<Citation>,
) -> ToolOutcome {
    let truncated = truncate_chars(content, MAX_SUBAGENT_RESULT_CHARS);
    let seconds = duration_ms / 1000;
    let summary = format!(
        "{} · {} · {nested_steps} 步 · {seconds}s",
        parsed.agent.label, parsed.description
    );
    let payload = render_task_payload(
        &parsed.task_id,
        &parsed.agent.name,
        "completed",
        &summary,
        &truncated,
    );
    let audit_fragment = format!(
        "子 Agent {} 完成：{} task_id={}",
        parsed.agent.name, parsed.description, parsed.task_id
    );
    task_outcome(
        "completed",
        args,
        summary,
        payload,
        citations,
        Some(audit_fragment),
    )
}

fn running_outcome(args: &Value, parsed: &ParsedTask) -> ToolOutcome {
    let summary = format!(
        "已在后台启动 {}：{}",
        parsed.agent.label, parsed.description
    );
    let text = [
        format!(
            "The task is working in the background (task_id: {}).",
            parsed.task_id
        ),
        "You will be notified automatically when it finishes.".to_owned(),
        "DO NOT sleep, poll for progress, or duplicate this task's work.".to_owned(),
    ]
    .join(" ");
    let payload = render_task_payload(
        &parsed.task_id,
        &parsed.agent.name,
        "running",
        &summary,
        &text,
    );
    task_outcome(
        "completed",
        args,
        summary,
        payload,
        Vec::new(),
        Some(format!("后台子 Agent {} 已启动", parsed.task_id)),
    )
}

fn failed_outcome(args: &Value, message: &str, task_id: Option<&str>) -> ToolOutcome {
    let payload = render_task_payload(
        task_id.unwrap_or("unknown"),
        "unknown",
        "error",
        message,
        message,
    );
    task_outcome(
        "failed",
        args,
        message.to_owned(),
        payload,
        Vec::new(),
        Some(format!("子 Agent 失败：{message}")),
    )
}

fn aborted_outcome(args: &Value, task_id: Option<&str>) -> ToolOutcome {
    let message = "用户中断了子 Agent。";
    let payload = render_task_payload(
        task_id.unwrap_or("unknown"),
        "unknown",
        "aborted",
        message,
        message,
    );
    task_outcome(
        "aborted",
        args,
        message.to_owned(),
        payload,
        Vec::new(),
        Some(message.to_owned()),
    )
}

fn task_outcome(
    status: &str,
    args: &Value,
    summary: String,
    payload: Value,
    citations: Vec<Citation>,
    audit_fragment: Option<String>,
) -> ToolOutcome {
    ToolOutcome {
        call: AgentToolCall {
            id: create_id("tool"),
            name: "task".to_owned(),
            status: status.to_owned(),
            summary,
            args: args.clone(),
        },
        payload,
        citations,
        audit_fragment,
    }
}

fn render_task_payload(
    task_id: &str,
    agent: &str,
    state: &str,
    summary: &str,
    text: &str,
) -> Value {
    let xml = format!(
        "<task id=\"{task_id}\" agent=\"{agent}\" state=\"{state}\">\n<summary>{summary}</summary>\n<task_result>\n{text}\n</task_result>\n</task>"
    );
    json!({
        "taskId": task_id,
        "agent": agent,
        "state": state,
        "summary": summary,
        "text": text,
        "content": xml
    })
}

fn format_background_notice(
    task_id: &str,
    agent: &str,
    description: &str,
    outcome: &ToolOutcome,
) -> String {
    let state = outcome
        .payload
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or(&outcome.call.status);
    let text = outcome
        .payload
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or(&outcome.call.summary);
    format!(
        "Background subagent {task_id} ({agent}: {description}) finished with state={state}.\nIts closing message:\n{text}"
    )
}

fn inbox_id(session_id: &str) -> String {
    format!("{INBOX_PREFIX}{session_id}")
}

fn memory_inbox() -> &'static Mutex<HashMap<String, Vec<String>>> {
    static INBOX: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    INBOX.get_or_init(|| Mutex::new(HashMap::new()))
}

fn push_notice(app: &AppHandle, session_id: &str, notice: String) {
    {
        let mut inbox = memory_inbox()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox
            .entry(session_id.to_owned())
            .or_default()
            .push(notice.clone());
    }
    let mut stored = load_persisted_notices(app, session_id);
    stored.push(json!({ "text": notice }));
    let _ = storage::save_agent_session_transcript(app, &inbox_id(session_id), &stored);
}

fn load_persisted_notices(app: &AppHandle, session_id: &str) -> Vec<Value> {
    storage::load_agent_session_transcript(app, &inbox_id(session_id))
        .ok()
        .flatten()
        .unwrap_or_default()
}

/** 取出父会话待注入的后台结算通知，内存和持久化一并清空。 */
pub(super) fn take_subagent_notices(app: &AppHandle, session_id: &str) -> Vec<String> {
    let mut notices = {
        let mut inbox = memory_inbox()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inbox.remove(session_id).unwrap_or_default()
    };
    if let Ok(Some(stored)) = storage::load_agent_session_transcript(app, &inbox_id(session_id)) {
        for item in stored {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if !notices.iter().any(|existing| existing == text) {
                    notices.push(text.to_owned());
                }
            } else if let Some(text) = item.as_str() {
                notices.push(text.to_owned());
            }
        }
        let _ = storage::delete_agent_session_transcript(app, &inbox_id(session_id));
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::super::subagent_personas::{build_subagent_system_prompt, resolve_agent};
    use super::*;
    use crate::agent_tools::ToolRegistry;
    use crate::domain::AgentSession;
    use serde_json::json;

    #[test]
    fn unknown_agent_fails_loud_with_available_names() {
        let error = parse_task_args(&json!({
            "description": "找笔记",
            "prompt": "列出认证相关笔记",
            "agent": "missing-role"
        }))
        .unwrap_err();
        assert!(error.contains("missing-role"));
        assert!(error.contains("explore"));
        assert!(error.contains("writer"));
    }

    #[test]
    fn missing_prompt_is_rejected() {
        let error = parse_task_args(&json!({
            "description": "找笔记",
            "agent": "explore"
        }))
        .unwrap_err();
        assert!(error.contains("prompt"));
    }

    #[test]
    fn parent_result_includes_task_id() {
        let parsed = parse_task_args(&json!({
            "description": "找认证笔记",
            "prompt": "列出登录相关笔记",
            "agent": "explore"
        }))
        .unwrap();
        let outcome = completed_outcome(
            &json!({ "description": "找认证笔记" }),
            &parsed,
            "结论：只有一篇登录笔记。",
            4,
            8000,
            Vec::new(),
        );
        let content = outcome.payload["content"].as_str().unwrap();
        assert!(content.contains("state=\"completed\""));
        assert!(content.contains(&parsed.task_id));
        assert!(outcome.payload["taskId"].as_str().is_some());
        assert!(outcome.call.summary.contains("探索"));
    }

    #[test]
    fn parallel_window_stops_at_writer_or_background() {
        let explore = json!({
            "description": "a",
            "prompt": "p",
            "agent": "explore"
        });
        let writer = json!({
            "description": "b",
            "prompt": "p",
            "agent": "writer"
        });
        let background = json!({
            "description": "c",
            "prompt": "p",
            "agent": "explore",
            "background": true
        });
        assert_eq!(
            parallel_window_size(&[
                explore.clone(),
                explore.clone(),
                explore.clone(),
                explore.clone()
            ]),
            3
        );
        assert_eq!(parallel_window_size(&[explore.clone(), writer]), 1);
        assert_eq!(parallel_window_size(&[explore, background]), 1);
    }

    #[test]
    fn fold_pending_change_merges_into_agent_direct_set() {
        let mut session = AgentSession {
            id: "session-a".to_owned(),
            title: "测试".to_owned(),
            im_identity: None,
            r#type: "knowledge-base".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: Vec::new(),
            pending_change: Some(ProposedChange {
                id: "change-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                note_id: Some("note-a".to_owned()),
                target_id: Some("note-a".to_owned()),
                target_kind: Some("note".to_owned()),
                file_type: Some("markdown".to_owned()),
                r#type: "rewrite".to_owned(),
                operation: Some("replace".to_owned()),
                title: "改写笔记".to_owned(),
                target_path: "Notes/a.md".to_owned(),
                original: "old".to_owned(),
                next: "new".to_owned(),
                original_hash: "hash".to_owned(),
                status: "pending".to_owned(),
                review_comments: None,
                review_state: None,
                diff_stats: None,
            }),
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
        };
        fold_pending_change_into_change_set(&mut session);
        assert!(session.pending_change.is_none());
        let set = session.pending_change_set.unwrap();
        assert_eq!(set.execution_id, AGENT_DIRECT_EXECUTION_ID);
        assert_eq!(set.operations.len(), 1);
        assert_eq!(set.operations[0].operation, "modify");
        assert_eq!(set.operations[0].target_path, "Notes/a.md");
    }

    #[test]
    fn writer_system_prompt_allows_edit_write() {
        use crate::domain::{FolderEntry, KnowledgeBase, Note, WorkspaceSnapshot};
        use crate::storage::hash_content;

        let snapshot = WorkspaceSnapshot {
            knowledge_bases: vec![KnowledgeBase {
                id: "kb-a".to_owned(),
                name: "主知识库".to_owned(),
                path: "/tmp/kb-a".to_owned(),
                description: "测试".to_owned(),
                status: "ready".to_owned(),
                note_count: 1,
                document_count: 0,
                updated_at: "刚刚".to_owned(),
                is_default: true,
                semantic_index_enabled: false,
                scan_report: None,
            }],
            folders: vec![FolderEntry {
                id: "folder-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                name: "Notes".to_owned(),
                path: "Notes".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            notes: vec![Note {
                id: "note-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                title: "授权笔记".to_owned(),
                path: "Notes/授权笔记.md".to_owned(),
                content_hash: hash_content("body"),
                content: "body".to_owned(),
                tags: Vec::new(),
                updated_at: "刚刚".to_owned(),
                backlinks: Vec::new(),
            }],
            documents: Vec::new(),
            sessions: vec![AgentSession {
                id: "session-a".to_owned(),
                title: "测试会话".to_owned(),
                im_identity: None,
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: Vec::new(),
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
        };
        let writer = resolve_agent("writer").unwrap();
        let prompt = build_subagent_system_prompt(&snapshot, 0, &writer);
        assert!(prompt.contains("edit / write"));
        assert!(prompt.contains("search, read, list, edit, write"));
        let names = ToolRegistry::for_subagent_tools(&writer.tools).tool_names();
        assert!(names.contains(&"edit"));
        assert!(names.contains(&"write"));
        assert!(!names.contains(&"task"));
    }

    #[test]
    fn depth_limit_constant_is_one() {
        assert_eq!(MAX_SUBAGENT_DEPTH, 1);
        assert_eq!(MAX_PARALLEL_SUBAGENTS, 3);
    }
}
