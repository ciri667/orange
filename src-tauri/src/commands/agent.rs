use super::common::*;

/** 运行 Agent 单轮 loop，检索作为工具由 Agent 自行选择。 */
#[tauri::command]
pub async fn run_agent_turn(
    app: AppHandle,
    payload: AgentTurnPayload,
) -> Result<AgentTurnResult, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let request = payload.request;
    let session_id = request.session_id.clone();
    let cancel = runtime::register_agent_cancel(&session_id);
    let _cancel_guard = runtime::AgentCancelGuard::new(session_id.clone(), cancel.clone());
    let mut snapshot = hydrate_persisted_sessions_for_turn(&app, payload.snapshot).await?;
    let active_knowledge_base_id = request.active_knowledge_base_id.clone();
    let request_metadata = json!({ "action": request.action.clone() });

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "run_agent_turn",
            "started",
            "Agent 开始处理用户请求。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone())
        .knowledge_base_id(active_knowledge_base_id.clone())
        .metadata(request_metadata),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let skills_app = app.clone();
    let available_skills = run_blocking("读取 Agent Skills", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::load_agent_skills(&skills_app, &connection)
    })
    .await?;

    // request 中的 active 信息来自 UI 当前焦点；会话 scope 已由 SQLite 中恢复的 session 决定。
    snapshot.active_knowledge_base_id = request.active_knowledge_base_id.clone();
    snapshot.active_note_id = request.active_note_id.clone();
    if snapshot
        .sessions
        .iter()
        .any(|session| session.id == request.session_id)
    {
        snapshot.active_session_id = request.session_id.clone();
    }

    let runtime_result =
        runtime::run_agent_turn(&app, snapshot, request, settings, available_skills, cancel).await;
    let audit_app = app.clone();
    let audit_log = runtime_result.audit_log.clone();

    if let Err(error) = run_blocking("写入请求审计日志", move || {
        storage::append_request_audit_log(&audit_app, &audit_log)
    })
    .await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "run_agent_turn",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .knowledge_base_id(active_knowledge_base_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }

    // 每轮后刷新本地索引，并只 upsert 本轮会话，确保消息可恢复且不覆盖其它会话。
    if let Err(error) =
        index_snapshot_in_background(app.clone(), &runtime_result.turn_result.snapshot).await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "run_agent_turn",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .knowledge_base_id(active_knowledge_base_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }
    if let Err(error) =
        persist_turn_session(&app, &runtime_result.turn_result.snapshot, &session_id).await
    {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "run_agent_turn",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .knowledge_base_id(active_knowledge_base_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "run_agent_turn",
            "completed",
            if runtime_result.audit_log.kind == "aborted_turn" {
                "用户中断了本轮 Agent 执行。"
            } else {
                "Agent 已完成本轮处理。"
            },
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .knowledge_base_id(active_knowledge_base_id)
        .duration(started_at.elapsed())
        .metadata(json!({
            "auditKind": runtime_result.audit_log.kind.clone(),
            "toolSummary": runtime_result.audit_log.tool_summary.clone(),
        })),
    );

    Ok(runtime_result.turn_result)
}

/** 用户手动中断当前会话的 Agent 回合；没有进行中的回合时是空操作。 */
#[tauri::command]
pub async fn abort_agent_turn(payload: AbortAgentTurnPayload) -> Result<(), String> {
    let session_id = payload.session_id.trim();
    if session_id.is_empty() {
        return Err("缺少会话 ID。".to_owned());
    }
    runtime::request_agent_abort(session_id);
    Ok(())
}

/** 手动整理当前 Agent 会话工作记忆，成功后持久化会话快照。 */
#[tauri::command]
pub async fn compact_agent_context(
    app: AppHandle,
    payload: CompactAgentContextPayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let session_id = payload.session_id.clone();

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "compact_agent_context",
            "started",
            "开始整理 Agent 会话上下文。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone()),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let snapshot = hydrate_persisted_sessions_for_turn(&app, payload.snapshot).await?;
    let snapshot =
        runtime::compact_agent_context_summary(&app, snapshot, &session_id, settings).await?;

    if let Err(error) = index_snapshot_in_background(app.clone(), &snapshot).await {
        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Error,
                AppLogCategory::Agent,
                "compact_agent_context",
                "failed",
                error.clone(),
            )
            .operation_id(operation_id)
            .session_id(session_id)
            .duration(started_at.elapsed()),
        );

        return Err(error);
    }
    persist_turn_session(&app, &snapshot, &session_id).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Agent,
            "compact_agent_context",
            "completed",
            "已整理 Agent 会话上下文。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .duration(started_at.elapsed()),
    );

    Ok(snapshot)
}

/** IM 后台入口复用 Agent runtime：创建或复用映射会话，持久化消息、审计和索引。 */
pub(crate) async fn run_agent_turn_from_im(
    app: AppHandle,
    provider_id: String,
    prompt: String,
    channel_key: String,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let operation_id = storage::create_id("op");
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 工作台状态", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let valid_scope_ids = snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| knowledge_base_ids.iter().any(|id| id == &knowledge_base.id))
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect::<Vec<_>>();

    if valid_scope_ids.is_empty() {
        return Err("IM 默认知识库范围为空或已失效。".to_owned());
    }

    let session_resolution = resolve_or_create_im_session(
        &app,
        &mut snapshot,
        &channel_key,
        &im_identity,
        valid_scope_ids.clone(),
    )?;
    let session_id = session_resolution.session_id;
    if let Some(pending_change) = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.pending_change.as_ref())
        .filter(|change| change.status == "pending")
    {
        // 一个 IM 会话只允许存在一个待确认变更，避免下一轮 Agent 覆盖远程用户尚未处理的 diff。
        return Err(format!(
            "当前有待确认变更 {}；请先发送“详情 {}”、“确认 {}”或“取消 {}”。",
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
            short_change_code(&pending_change.id),
        ));
    }
    let active_knowledge_base_id = valid_scope_ids.first().cloned().unwrap_or_default();
    let user_message = crate::im::build_im_user_message(&prompt);
    let user_message_id = user_message.id.clone();

    if let Some(session) = snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        session.messages.push(user_message);
        session.updated_at = storage::format_local_datetime();
    }

    snapshot.active_session_id = session_id.clone();
    snapshot.active_knowledge_base_id = active_knowledge_base_id.clone();
    snapshot.active_note_id.clear();
    snapshot.active_document_id.clear();

    storage::save_snapshot_session(&app, &snapshot, &session_id)?;

    logging::write_app_event_best_effort(
        &app,
        build_im_identity_event(
            &session_id,
            &im_identity,
            session_resolution.identity_status,
        ),
    );

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_agent_turn",
            "started",
            "IM 消息开始进入 Agent runtime。",
        )
        .operation_id(operation_id.clone())
        .session_id(session_id.clone())
        .knowledge_base_id(active_knowledge_base_id.clone())
        .metadata(json!({
            "providerId": provider_id.clone(),
            "scopeCount": valid_scope_ids.len(),
            "promptChars": prompt.chars().count(),
            "channelHash": storage::hash_content(&channel_key).chars().take(16).collect::<String>(),
        })),
    );

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let skills_app = app.clone();
    let available_skills = run_blocking("读取 Agent Skills", move || {
        let connection = storage::open_database(&skills_app)?;

        skills::load_agent_skills(&skills_app, &connection)
    })
    .await?;
    let request = crate::im::build_im_turn_request(
        prompt,
        session_id.clone(),
        active_knowledge_base_id.clone(),
        user_message_id,
    );
    let cancel = runtime::register_agent_cancel(&session_id);
    let _cancel_guard = runtime::AgentCancelGuard::new(session_id.clone(), cancel.clone());
    let runtime_result =
        runtime::run_agent_turn(&app, snapshot, request, settings, available_skills, cancel).await;
    let audit_app = app.clone();
    let audit_log = runtime_result.audit_log.clone();

    run_blocking("写入 IM 请求审计日志", move || {
        storage::append_request_audit_log(&audit_app, &audit_log)
    })
    .await?;
    index_snapshot_in_background(app.clone(), &runtime_result.turn_result.snapshot).await?;
    persist_turn_session(&app, &runtime_result.turn_result.snapshot, &session_id).await?;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            AppLogLevel::Info,
            AppLogCategory::Im,
            "im_agent_turn",
            "completed",
            "IM 消息已完成 Agent runtime 处理。",
        )
        .operation_id(operation_id)
        .session_id(session_id)
        .knowledge_base_id(active_knowledge_base_id)
        .duration(started_at.elapsed())
        .metadata(json!({
            "providerId": provider_id,
            "auditKind": runtime_result.audit_log.kind,
            "toolSummary": runtime_result.audit_log.tool_summary,
        })),
    );

    Ok(runtime_result.turn_result.snapshot)
}

/**
 * 处理 provider 无关的 IM 内置指令。调用方必须先完成鉴权、去重与群聊 @ 门禁；
 * 本入口仅从持久化状态读取会话，避免远端事件携带过期的工作台快照覆盖本地数据。
 */
pub(crate) async fn handle_im_builtin_command(
    app: AppHandle,
    provider_id: &str,
    command: crate::im::ImBuiltinCommand,
    channel_key: &str,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> String {
    let started_at = Instant::now();
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let command_name = match command {
        crate::im::ImBuiltinCommand::Help => "help",
        crate::im::ImBuiltinCommand::New => "new",
        crate::im::ImBuiltinCommand::Compact => "compact",
    };

    let result = match command {
        crate::im::ImBuiltinCommand::Help => Ok(ImBuiltinCommandResult {
            reply: crate::im::builtin_command_help_text().to_owned(),
            session_id: None,
            message_count: None,
            summary_chars: None,
        }),
        crate::im::ImBuiltinCommand::New => {
            create_im_session_from_command(
                &app,
                provider_id,
                channel_key,
                knowledge_base_ids,
                im_identity,
            )
            .await
        }
        crate::im::ImBuiltinCommand::Compact => {
            compact_im_session_from_command(&app, provider_id, channel_key).await
        }
    };

    let (status, reply, session_id, message_count, summary_chars) = match result {
        Ok(result) => (
            "completed",
            result.reply,
            result.session_id,
            result.message_count,
            result.summary_chars,
        ),
        Err(error) => (
            "failed",
            format!("操作失败：{}", logging::sanitize_log_text(&error)),
            None,
            None,
            None,
        ),
    };
    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if status == "failed" {
                AppLogLevel::Warn
            } else {
                AppLogLevel::Info
            },
            AppLogCategory::Im,
            "im_builtin_command",
            status,
            "IM 内置指令已处理。",
        )
        .session_id(session_id.unwrap_or_default())
        .duration(started_at.elapsed())
        .metadata(json!({
            "command": command_name,
            "providerId": provider_id,
            "channelHash": channel_hash,
            "messageCount": message_count,
            "summaryChars": summary_chars,
        })),
    );
    reply
}

/** 内置指令的用户可见结果和可观测性指标；不包含消息正文或摘要内容。 */
pub(super) struct ImBuiltinCommandResult {
    reply: String,
    session_id: Option<String>,
    message_count: Option<usize>,
    summary_chars: Option<usize>,
}

/** 创建并立即映射空 IM 会话；有待确认文件变更时拒绝切换，保留远程审批入口。 */
pub(super) async fn create_im_session_from_command(
    app: &AppHandle,
    _provider_id: &str,
    channel_key: &str,
    knowledge_base_ids: Vec<String>,
    im_identity: crate::domain::ImSessionIdentity,
) -> Result<ImBuiltinCommandResult, String> {
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 新会话状态", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let valid_scope_ids = snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| knowledge_base_ids.iter().any(|id| id == &knowledge_base.id))
        .map(|knowledge_base| knowledge_base.id.clone())
        .collect::<Vec<_>>();
    if valid_scope_ids.is_empty() {
        return Err("IM 默认知识库范围为空或已失效。".to_owned());
    }

    if let Some(mapped_id) = storage::load_im_session_mapping(app, channel_key)? {
        if snapshot.sessions.iter().any(|session| {
            session.id == mapped_id
                && session
                    .pending_change
                    .as_ref()
                    .is_some_and(|change| change.status == "pending")
        }) {
            return Ok(ImBuiltinCommandResult {
                reply:
                    "当前会话有待确认变更；请先发送“详情 <编号>”、“确认 <编号>”或“取消 <编号>”。"
                        .to_owned(),
                session_id: Some(mapped_id),
                message_count: None,
                summary_chars: None,
            });
        }
    }

    let new_identity = crate::im::build_im_new_session_identity(&im_identity);
    // 明确使用固定摘要，保证命令本身不会成为新会话的标题或可见主题。
    let session = crate::im::build_im_agent_session(new_identity, valid_scope_ids);
    let session_id = session.id.clone();
    snapshot.sessions.insert(0, session);
    storage::save_snapshot_session(app, &snapshot, &session_id)?;
    storage::save_im_session_mapping(app, channel_key, &session_id)?;

    Ok(ImBuiltinCommandResult {
        reply: "已开启新会话，下一条消息将从新的上下文开始。".to_owned(),
        session_id: Some(session_id),
        message_count: Some(0),
        summary_chars: Some(0),
    })
}

/** 压缩当前 channel 的持久化会话；空会话直接提示，绝不为此调用模型。 */
pub(super) async fn compact_im_session_from_command(
    app: &AppHandle,
    provider_id: &str,
    channel_key: &str,
) -> Result<ImBuiltinCommandResult, String> {
    let Some(session_id) = storage::load_im_session_mapping(app, channel_key)? else {
        return Ok(ImBuiltinCommandResult {
            reply: "当前还没有可整理的会话；请先发送一条普通消息。".to_owned(),
            session_id: None,
            message_count: Some(0),
            summary_chars: Some(0),
        });
    };
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载 IM 上下文", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let Some(session_index) = snapshot.sessions.iter().position(|session| {
        session.id == session_id
            && session.im_identity.as_ref().is_some_and(|identity| {
                identity.provider_id == provider_id && identity.channel_hash == channel_hash
            })
    }) else {
        return Ok(ImBuiltinCommandResult {
            reply: "当前 IM 会话不可用，请发送 /new 开启新会话。".to_owned(),
            session_id: Some(session_id),
            message_count: None,
            summary_chars: None,
        });
    };
    let message_count = snapshot.sessions[session_index].messages.len();
    if message_count == 0 {
        return Ok(ImBuiltinCommandResult {
            reply: "当前会话没有可整理的对话消息。".to_owned(),
            session_id: Some(session_id),
            message_count: Some(0),
            summary_chars: Some(0),
        });
    }

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    // 模型调用不可用时压缩为确定性工作记忆，确保移动端命令始终能完成并被持久化。
    snapshot = match runtime::compact_agent_context_summary(
        &app,
        snapshot.clone(),
        &session_id,
        settings,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            log::warn!(target: "im", "IM 上下文压缩降级为确定性摘要：session={} reason={}", session_id, logging::sanitize_log_text(&error));
            runtime::update_agent_context_summary_deterministic(
                &mut snapshot,
                session_index,
                Some(&error),
                true,
            );
            snapshot
        }
    };
    let summary = snapshot.sessions[session_index].context_summary.as_ref();
    let summary_chars = summary
        .map(|item| {
            serde_json::to_string(item)
                .unwrap_or_default()
                .chars()
                .count()
        })
        .unwrap_or(0);
    let goal = summary
        .and_then(|item| item.current_goal.as_deref())
        .unwrap_or("未识别当前目标");
    // 压缩摘要可能复述用户输入；回传到群聊前沿用日志脱敏规则，避免意外暴露密钥或本地绝对路径。
    let short_goal = logging::sanitize_log_text(goal)
        .chars()
        .take(48)
        .collect::<String>();
    storage::save_snapshot_session(app, &snapshot, &session_id)?;
    index_snapshot_in_background(app.clone(), &snapshot).await?;

    Ok(ImBuiltinCommandResult {
        reply: format!(
            "已整理当前会话上下文（{} 条消息）。当前目标：{}",
            message_count, short_goal
        ),
        session_id: Some(session_id),
        message_count: Some(message_count),
        summary_chars: Some(summary_chars),
    })
}

/**
 * 在 IM 会话内处理待确认变更；此入口只信任持久化的会话和 channel 映射，
 * 不接受桌面前端传入的 WorkspaceSnapshot，避免远程确认覆盖本地最新状态。
 */
pub(crate) async fn handle_im_pending_change_command(
    app: AppHandle,
    provider_id: &str,
    channel_key: &str,
    action: &str,
    change_code: &str,
) -> String {
    let started_at = Instant::now();
    let channel_hash = storage::hash_content(channel_key)
        .chars()
        .take(16)
        .collect::<String>();
    let normalized_action = action.trim().to_ascii_lowercase();
    let normalized_code = change_code.trim();

    // 短编号至少六位，既方便手机端输入，也避免将空字符串或过短前缀匹配到其他变更。
    if normalized_code.chars().count() < 6 {
        return "变更编号至少需要 6 位，例如：确认 change-123456。".to_owned();
    }
    if !matches!(normalized_action.as_str(), "confirm" | "cancel" | "details") {
        return "不支持的变更操作。请使用：确认 <编号>、取消 <编号> 或详情 <编号>。".to_owned();
    }

    let snapshot_app = app.clone();
    let mut snapshot = match run_blocking("加载 IM 待确认变更", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => return format!("读取待确认变更失败：{}", logging::sanitize_log_text(&error)),
    };
    let session_id = match storage::load_im_session_mapping(&app, channel_key) {
        Ok(Some(session_id)) => session_id,
        Ok(None) => return "当前 IM 会话没有待确认变更。".to_owned(),
        Err(error) => return format!("读取 IM 会话失败：{}", logging::sanitize_log_text(&error)),
    };
    let Some(session_index) = snapshot.sessions.iter().position(|session| {
        session.id == session_id
            // 群聊 channel key 已包含发送人 hash；再次匹配持久化身份，确保其他成员不能确认该变更。
            && session.im_identity.as_ref().is_some_and(|identity| {
                identity.provider_id == provider_id && identity.channel_hash == channel_hash
            })
    }) else {
        log_im_change_approval(
            &app,
            "blocked",
            &normalized_action,
            None,
            &channel_hash,
            started_at,
        );
        return "当前身份无权处理该待确认变更。".to_owned();
    };
    let Some(change) = snapshot.sessions[session_index].pending_change.clone() else {
        return "当前 IM 会话没有待确认变更。".to_owned();
    };
    if change.status != "pending" {
        return format!(
            "变更 {} 已处理（状态：{}），无需重复操作。",
            short_change_code(&change.id),
            change.status
        );
    }
    if !change.id.eq_ignore_ascii_case(normalized_code)
        && !change
            .id
            .to_ascii_lowercase()
            .starts_with(&normalized_code.to_ascii_lowercase())
    {
        return "找不到该编号对应的待确认变更；请检查编号后重试。".to_owned();
    }

    if normalized_action == "details" {
        log_im_change_approval(
            &app,
            "completed",
            "details",
            Some(&change.id),
            &channel_hash,
            started_at,
        );
        return build_im_change_details(&change);
    }

    snapshot.active_session_id = session_id.clone();
    let mut snapshot_before_apply = snapshot.clone();
    let result = if normalized_action == "cancel" {
        reject_proposed_change(app.clone(), ChangePayload { snapshot }).await
    } else {
        apply_proposed_change(app.clone(), ChangePayload { snapshot }).await
    };

    match result {
        Ok(updated_snapshot) => {
            // IM 操作没有前端接收 snapshot，因此必须立即持久化会话状态，重复消息才不会二次写入。
            if let Err(error) = storage::save_snapshot_session(&app, &updated_snapshot, &session_id)
            {
                log_im_change_approval(
                    &app,
                    "failed",
                    &normalized_action,
                    Some(&change.id),
                    &channel_hash,
                    started_at,
                );
                return format!(
                    "变更已处理，但保存会话状态失败：{}",
                    logging::sanitize_log_text(&error)
                );
            }
            log_im_change_approval(
                &app,
                "completed",
                &normalized_action,
                Some(&change.id),
                &channel_hash,
                started_at,
            );
            if normalized_action == "cancel" {
                format!(
                    "已取消变更 {}，本地文件未修改。",
                    short_change_code(&change.id)
                )
            } else {
                format!("已确认并写入变更 {}。", short_change_code(&change.id))
            }
        }
        Err(error) => {
            // 冲突不可重试且不能保留为可确认状态；写入失败则保留 pending，允许用户稍后处理。
            if error.contains("已变化") || error.contains("未命中") || error.contains("出现多次")
            {
                if let Some(session) = snapshot_before_apply.sessions.get_mut(session_index) {
                    if let Some(pending_change) = session.pending_change.as_mut() {
                        pending_change.status = "expired".to_owned();
                    }
                    session.updated_at = storage::format_local_datetime();
                }
                let _ = storage::save_snapshot_session(&app, &snapshot_before_apply, &session_id);
                log_im_change_approval(
                    &app,
                    "conflict",
                    &normalized_action,
                    Some(&change.id),
                    &channel_hash,
                    started_at,
                );
                return "变更已过期，请重新生成；本地文件未被覆盖。".to_owned();
            }
            log_im_change_approval(
                &app,
                "failed",
                &normalized_action,
                Some(&change.id),
                &channel_hash,
                started_at,
            );
            format!("处理变更失败：{}", logging::sanitize_log_text(&error))
        }
    }
}

/** 返回 IM 详情的截断 diff，正文只在用户显式请求时发送。 */
pub(crate) fn build_im_change_details(change: &ProposedChange) -> String {
    // 飞书按 UTF-8 字节计入消息体；这里同时控制中文场景的实际传输体积。
    const MAX_DIFF_CHARS: usize = 700;
    let original = change
        .original
        .chars()
        .take(MAX_DIFF_CHARS / 2)
        .collect::<String>();
    let next = change
        .next
        .chars()
        .take(MAX_DIFF_CHARS / 2)
        .collect::<String>();
    format!(
        "变更 {} 详情\n目标：{}\n类型：{}\n\n--- 原内容 ---\n{}\n\n+++ 建议内容 +++\n{}\n\n回复“确认 {}”写入，或“取消 {}”放弃。",
        short_change_code(&change.id), change.target_path, change.r#type, original, next,
        short_change_code(&change.id), short_change_code(&change.id)
    )
}

/** 生成稳定、可输入的变更短编号；完整 ID 仍只保存在本地会话中。 */
pub(crate) fn short_change_code(change_id: &str) -> String {
    change_id.chars().take(12).collect()
}

/** 记录 IM 审批审计，只写入变更 ID、通道 hash、动作和结果，不记录正文或外部原始身份。 */
pub(super) fn log_im_change_approval(
    app: &AppHandle,
    status: &str,
    action: &str,
    change_id: Option<&str>,
    channel_hash: &str,
    started_at: Instant,
) {
    logging::write_app_event_best_effort(
        app,
        AppEventBuilder::new(
            if matches!(status, "failed" | "conflict") {
                AppLogLevel::Warn
            } else {
                AppLogLevel::Info
            },
            AppLogCategory::Im,
            "im_change_approval",
            status,
            "IM 待确认变更操作已处理。",
        )
        .entity("change", change_id.unwrap_or("unknown"))
        .duration(started_at.elapsed())
        .metadata(json!({ "action": action, "channelHash": channel_hash })),
    );
}

/** 为 IM channel 找到已有会话；不存在或失效时创建一个新的 AgentSession 并保存映射。 */
pub(super) fn resolve_or_create_im_session(
    app: &AppHandle,
    snapshot: &mut WorkspaceSnapshot,
    channel_key: &str,
    im_identity: &crate::domain::ImSessionIdentity,
    knowledge_base_ids: Vec<String>,
) -> Result<ImSessionResolution, String> {
    let mapped_session_id = storage::load_im_session_mapping(app, channel_key)?;

    if let Some(session_id) = mapped_session_id {
        if let Some(session) = snapshot
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.knowledge_base_ids = knowledge_base_ids;
            session.updated_at = storage::format_local_datetime();
            let identity_status = if session.im_identity.is_some() {
                // 已有 IM 身份时只更新最近消息摘要，稳定标题不能被后续消息覆盖。
                if let Some(existing_identity) = &mut session.im_identity {
                    existing_identity.last_message_preview =
                        im_identity.last_message_preview.clone();
                }
                "updated"
            } else {
                // 旧版映射会话首次再次收到 IM 消息时补齐完整身份和标题。
                session.im_identity = Some(im_identity.clone());
                session.title = crate::im::format_im_session_title(im_identity);
                "migrated"
            };

            return Ok(ImSessionResolution {
                session_id: session.id.clone(),
                identity_status,
            });
        }
    }

    let session = crate::im::build_im_agent_session(im_identity.clone(), knowledge_base_ids);
    let session_id = session.id.clone();

    snapshot.sessions.insert(0, session);
    storage::save_im_session_mapping(app, channel_key, &session_id)?;

    Ok(ImSessionResolution {
        session_id,
        identity_status: "created",
    })
}

/** IM 会话创建、迁移和更新的结果；用于统一写入轻量脱敏身份审计。 */
pub(super) struct ImSessionResolution {
    session_id: String,
    identity_status: &'static str,
}

/** 构造可观测但不包含消息正文和外部原始 ID 的 IM 身份日志。 */
pub(crate) fn build_im_identity_event(
    session_id: &str,
    identity: &crate::domain::ImSessionIdentity,
    status: &str,
) -> AppEventBuilder {
    AppEventBuilder::new(
        AppLogLevel::Info,
        AppLogCategory::Im,
        "im_session_identity",
        status,
        "IM 会话身份已同步。",
    )
    .session_id(session_id)
    .metadata(json!({
        "providerId": identity.provider_id,
        "conversationKind": identity.conversation_kind,
        "channelHash": identity.channel_hash,
        "initialPreviewChars": identity.initial_message_preview.chars().count(),
        "lastPreviewChars": identity.last_message_preview.chars().count(),
        "isFallback": identity.conversation_kind == "unknown",
    }))
}

/** 确认待写入 diff，校验知识库边界和内容 hash 后原子写回 Markdown。 */
#[tauri::command]
pub async fn apply_proposed_change(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let apply_app = app.clone();
    let mut snapshot = run_blocking("应用 Agent 变更", move || {
        crate::agent_writes::apply_pending_change(&apply_app, payload.snapshot)
    })
    .await?;
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;
    runtime::update_agent_context_summary_deterministic(&mut snapshot, session_index, None, false);
    Ok(snapshot)
}

/** 拒绝待写入 diff，只更新会话状态，不修改任何 Markdown 文件。 */
#[tauri::command]
pub async fn reject_proposed_change(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let started_at = Instant::now();
    let mut snapshot = payload.snapshot;
    let session_id = snapshot.active_session_id.clone();
    let session_index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话".to_owned())?;

    if let Some(change) = snapshot.sessions[session_index].pending_change.clone() {
        let rejected_change_id = change.id.clone();
        let rejected_change_type = change.r#type.clone();
        let rejected_operation = change.operation.clone();
        let rejected_review_comment_count = change
            .review_comments
            .as_ref()
            .map(|comments| comments.len())
            .unwrap_or_default();
        let rejected_diff_hunk_count = change.diff_stats.as_ref().map(|stats| stats.hunk_count);
        let rejected_knowledge_base_id = change.knowledge_base_id.clone();
        let rejected_target_path = change.target_path.clone();

        snapshot.sessions[session_index].pending_change = Some(crate::domain::ProposedChange {
            status: "rejected".to_owned(),
            ..change
        });
        snapshot.sessions[session_index].updated_at = "刚刚".to_owned();
        runtime::update_agent_context_summary_deterministic(
            &mut snapshot,
            session_index,
            None,
            false,
        );

        logging::write_app_event_best_effort(
            &app,
            AppEventBuilder::new(
                AppLogLevel::Info,
                AppLogCategory::Agent,
                "reject_proposed_change",
                "completed",
                "已拒绝 Agent diff。",
            )
            .session_id(&session_id)
            .knowledge_base_id(rejected_knowledge_base_id)
            .entity("change", rejected_change_id)
            .relative_path(rejected_target_path)
            .duration(started_at.elapsed())
            .metadata(json!({
                "changeType": rejected_change_type,
                "operation": rejected_operation,
                "reviewCommentCount": rejected_review_comment_count,
                "diffHunkCount": rejected_diff_hunk_count,
            })),
        );
    }

    index_snapshot_in_background(app.clone(), &snapshot).await?;
    persist_turn_session(&app, &snapshot, &session_id).await?;

    Ok(snapshot)
}

/** 批准待执行 Skill；命令在线程池中完成隔离副本、沙箱执行和变更集生成。 */
#[tauri::command]
pub async fn approve_skill_execution(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let execution_app = app.clone();
    let started_at = Instant::now();
    let session_id = payload.snapshot.active_session_id.clone();
    let result = run_blocking("执行已批准 Skill", move || {
        skill_execution::approve_and_execute(&execution_app, payload.snapshot)
    })
    .await;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if result.is_ok() {
                AppLogLevel::Info
            } else {
                AppLogLevel::Warn
            },
            AppLogCategory::Agent,
            "approve_skill_execution",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            if result.is_ok() {
                "Skill 隔离执行完成。"
            } else {
                "Skill 隔离执行失败。"
            },
        )
        .session_id(session_id)
        .duration(started_at.elapsed()),
    );
    result
}

/** 拒绝待执行 Skill；不创建隔离副本，也不启动进程。 */
#[tauri::command]
pub async fn reject_skill_execution(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let rejection_app = app.clone();
    let session_id = payload.snapshot.active_session_id.clone();
    let result = run_blocking("拒绝 Skill 执行", move || {
        skill_execution::reject_execution(&rejection_app, payload.snapshot)
    })
    .await;

    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if result.is_ok() {
                AppLogLevel::Info
            } else {
                AppLogLevel::Warn
            },
            AppLogCategory::Agent,
            "reject_skill_execution",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            if result.is_ok() {
                "已拒绝 Skill 执行。"
            } else {
                "拒绝 Skill 执行失败。"
            },
        )
        .session_id(session_id),
    );
    result
}

/** 应用当前 Skill 变更集；后端负责全量预检、原子写入与失败回滚。 */
#[tauri::command]
pub async fn apply_skill_change_set(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let change_app = app.clone();
    let started_at = Instant::now();
    let session_id = payload.snapshot.active_session_id.clone();
    let result = run_blocking("应用 Skill 变更集", move || {
        skill_execution::apply_change_set(&change_app, payload.snapshot)
    })
    .await;
    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if result.is_ok() {
                AppLogLevel::Info
            } else {
                AppLogLevel::Warn
            },
            AppLogCategory::Agent,
            "apply_skill_change_set",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            if result.is_ok() {
                "已应用 Skill 文件变更集。"
            } else {
                "应用 Skill 文件变更集失败。"
            },
        )
        .session_id(session_id)
        .duration(started_at.elapsed()),
    );
    result
}

/** 拒绝当前 Skill 变更集并清理隔离副本。 */
#[tauri::command]
pub async fn reject_skill_change_set(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let change_app = app.clone();
    run_blocking("拒绝 Skill 变更集", move || {
        skill_execution::reject_change_set(&change_app, payload.snapshot)
    })
    .await
}

/** 应用当前 Agent 直接产出的变更集（无 Skill 执行隔离区）；同样走全量预检、原子写入与回滚。 */
#[tauri::command]
pub async fn apply_agent_change_set(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let change_app = app.clone();
    let started_at = Instant::now();
    let session_id = payload.snapshot.active_session_id.clone();
    let result = run_blocking("应用 Agent 变更集", move || {
        skill_execution::apply_agent_change_set(&change_app, payload.snapshot)
    })
    .await;
    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if result.is_ok() {
                AppLogLevel::Info
            } else {
                AppLogLevel::Warn
            },
            AppLogCategory::Agent,
            "apply_agent_change_set",
            if result.is_ok() {
                "completed"
            } else {
                "failed"
            },
            if result.is_ok() {
                "已应用 Agent 文件变更集。"
            } else {
                "应用 Agent 文件变更集失败。"
            },
        )
        .session_id(session_id)
        .duration(started_at.elapsed()),
    );
    result
}

/** 拒绝当前 Agent 变更集；只清空待确认状态，不触碰 Skill 隔离目录。 */
#[tauri::command]
pub async fn reject_agent_change_set(
    app: AppHandle,
    payload: ChangePayload,
) -> Result<WorkspaceSnapshot, String> {
    let change_app = app.clone();
    run_blocking("拒绝 Agent 变更集", move || {
        skill_execution::reject_agent_change_set(&change_app, payload.snapshot)
    })
    .await
}

/** Agent turn 前合并 SQLite 中的持久化会话，避免模型或规则 Agent 只信任前端传入的 scope 快照。 */
pub(super) async fn hydrate_persisted_sessions_for_turn(
    app: &AppHandle,
    mut snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let sessions_app = app.clone();
    let snapshot_for_sessions = snapshot.clone();
    let persisted_sessions = run_blocking("读取持久化 Agent 会话", move || {
        storage::load_sessions_for_snapshot(&sessions_app, &snapshot_for_sessions)
    })
    .await?;

    if !persisted_sessions.is_empty() {
        snapshot.sessions = persisted_sessions;
    }

    storage::normalize_sessions_for_snapshot(&mut snapshot);

    if !snapshot
        .sessions
        .iter()
        .any(|session| session.id == snapshot.active_session_id)
    {
        snapshot.active_session_id = snapshot
            .sessions
            .first()
            .map(|session| session.id.clone())
            .unwrap_or_default();
    }

    Ok(snapshot)
}
