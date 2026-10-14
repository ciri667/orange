use super::common::*;
use crate::builtin_commands::{self, CommandScope};

/** 桌面端 `/` 弹窗的命令候选；只返回公开元数据，不含处理函数或内部路径。 */
#[tauri::command]
pub async fn list_builtin_commands() -> Result<Vec<BuiltinCommandDescriptor>, String> {
    Ok(
        builtin_commands::builtin_commands_for_scope(CommandScope::Desktop)
            .into_iter()
            .copied()
            .collect(),
    )
}

/**
 * 执行桌面端内置指令。
 *
 * 指令是本地动作，绝不进入模型上下文：这里不会构造 AgentTurnRequest，
 * 需要落盘的指令直接复用 storage 的会话写入路径。
 * 未知 command_id 返回错误，由调用方决定是否回退为普通消息。
 */
#[tauri::command]
pub async fn run_builtin_command(
    app: AppHandle,
    payload: RunBuiltinCommandPayload,
) -> Result<BuiltinCommandRunResult, String> {
    let started_at = Instant::now();
    let command_id = payload.command_id.clone();
    let command = builtin_commands::find_builtin_command(&command_id)
        .ok_or_else(|| format!("未知的内置指令：{command_id}"))?;

    if !command.scopes.contains(&CommandScope::Desktop) {
        return Err(format!("内置指令 {command_id} 不支持桌面端入口。"));
    }

    let session_id = payload.snapshot.active_session_id.clone();
    // args 可能包含会话标题等用户内容，因此只记录长度，不记录正文。
    let args_length = payload.args.chars().count();
    // requires_active_session 的指令在草稿会话下没有可落盘的会话记录，必须先提示用户。
    if command.requires_active_session {
        let has_persisted_session = !session_id.is_empty()
            && payload.snapshot.sessions.iter().any(|session| {
                // 草稿会话不在快照列表中，因此能在这里找到即代表已落盘。
                session.id == session_id
            });
        if !has_persisted_session {
            return Err("当前还没有已保存的会话，请先发送一条消息再使用该指令。".to_owned());
        }
    }

    let result = match command.id {
        "help" => Ok(BuiltinCommandRunResult {
            command_id: command_id.clone(),
            notice: builtin_commands::builtin_command_help_text(CommandScope::Desktop),
            snapshot: None,
        }),
        "status" => Ok(BuiltinCommandRunResult {
            command_id: command_id.clone(),
            notice: build_session_status_notice(&payload.snapshot),
            snapshot: None,
        }),
        "new" => create_session_from_command(&app).await,
        "rename" => rename_session_from_command(&app, payload.snapshot, payload.args).await,
        "compact" => compact_session_from_command(&app, payload.snapshot).await,
        _ => Err(format!("内置指令 {command_id} 尚未实现。")),
    };

    let (status, log_reason) = match &result {
        Ok(_) => ("completed", "桌面端内置指令已处理。".to_owned()),
        Err(error) => ("failed", logging::sanitize_log_text(error)),
    };
    // args 可能包含会话标题等用户内容，因此只记录长度，不记录正文。
    logging::write_app_event_best_effort(
        &app,
        AppEventBuilder::new(
            if status == "failed" {
                AppLogLevel::Warn
            } else {
                AppLogLevel::Info
            },
            AppLogCategory::Agent,
            "desktop_builtin_command",
            status,
            log_reason,
        )
        .session_id(session_id)
        .duration(started_at.elapsed())
        .metadata(json!({
            "command": command_id,
            "argsLength": args_length,
        })),
    );

    result
}

/** 构造 `/status` 的本地状态文案；只展示会话、模型与上下文占用，不含正文。 */
fn build_session_status_notice(snapshot: &WorkspaceSnapshot) -> String {
    let Some(session) = snapshot
        .sessions
        .iter()
        .find(|session| session.id == snapshot.active_session_id)
    else {
        return "当前没有活跃会话。".to_owned();
    };

    let model = match (
        session.model_provider_id.as_deref(),
        session.model_id.as_deref(),
    ) {
        (Some(provider_id), Some(model_id)) if !model_id.is_empty() => {
            format!("{provider_id}/{model_id}")
        }
        _ => "跟随全局默认模型".to_owned(),
    };
    let context = session
        .context_usage
        .as_ref()
        .map(|usage| {
            format!(
                "{} tokens（模型窗口 {}）",
                usage.total_tokens,
                usage
                    .context_length
                    .map(|length| length.to_string())
                    .unwrap_or_else(|| "未知".to_owned())
            )
        })
        .unwrap_or_else(|| "暂无记录".to_owned());

    format!(
        "会话：{}\n消息：{} 条\n模型：{}\n上下文：{}\n安全级别：{}\n计划模式：{}",
        session.title,
        session.messages.len(),
        model,
        context,
        session.security_level,
        if session.plan_mode {
            "开启"
        } else {
            "关闭"
        }
    )
}

/** 构造 `/new` 的空白会话；标题使用占位值，等首条输入接管。 */
fn build_new_command_session(knowledge_base_id: &str) -> AgentSession {
    let now = storage::format_local_datetime();

    AgentSession {
        id: storage::create_id("session"),
        // 必须使用占位标题：首条用户输入据此接管标题，留空会导致会话永远没有标题。
        title: storage::DEFAULT_SESSION_TITLE.to_owned(),
        im_identity: None,
        schedule_identity: None,
        r#type: "knowledge-base".to_owned(),
        knowledge_base_ids: vec![knowledge_base_id.to_owned()],
        active_note_id: None,
        pinned_note_ids: Vec::new(),
        messages: Vec::new(),
        pending_change: None,
        pending_change_set: None,
        pending_execution: None,
        security_level: "basic".to_owned(),
        plan_mode: false,
        context_summary: None,
        created_at: now.clone(),
        updated_at: now,
        deleted_at: None,
        model_provider_id: None,
        model_id: None,
        context_usage: None,
        title_customized: false,
        pinned_at: None,
        archived_at: None,
    }
}

/** 新建空白会话并直接成为活跃会话；标题先占位，由首条输入补齐。 */
async fn create_session_from_command(app: &AppHandle) -> Result<BuiltinCommandRunResult, String> {
    let snapshot_app = app.clone();
    let mut snapshot = run_blocking("加载工作台快照", move || {
        storage::load_workspace_snapshot(&snapshot_app)
    })
    .await?;

    let knowledge_base_id = snapshot.active_knowledge_base_id.clone();
    if knowledge_base_id.is_empty() {
        return Err("当前没有可用的知识库，无法新建 Agent 会话。".to_owned());
    }

    let session = build_new_command_session(&knowledge_base_id);
    let session_id = session.id.clone();

    snapshot.sessions.insert(0, session);
    snapshot.active_session_id = session_id.clone();
    let persist_app = app.clone();
    let persist_snapshot = snapshot.clone();
    let persist_session_id = session_id.clone();
    run_blocking("保存新会话", move || {
        storage::save_snapshot_session(&persist_app, &persist_snapshot, &persist_session_id)
    })
    .await?;

    Ok(BuiltinCommandRunResult {
        command_id: "new".to_owned(),
        notice: "已开启新会话，下一条消息将从新的上下文开始。".to_owned(),
        snapshot: Some(snapshot),
    })
}

/** 重命名当前会话并标记为自定义标题，避免后续首条消息覆盖用户命名。 */
async fn rename_session_from_command(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
    args: String,
) -> Result<BuiltinCommandRunResult, String> {
    let title = args.trim().to_owned();
    if title.is_empty() {
        return Err("用法：/rename <标题>".to_owned());
    }

    let session_id = snapshot.active_session_id.clone();
    let mut next_snapshot = snapshot;
    let Some(session) = next_snapshot
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    else {
        return Err("找不到要重命名的会话。".to_owned());
    };
    session.title = title.clone();
    session.title_customized = true;
    session.updated_at = storage::format_local_datetime();

    let save_app = app.clone();
    let save_snapshot = next_snapshot.clone();
    let save_session_id = session_id.clone();
    let saved = run_blocking("保存会话标题", move || {
        storage::save_snapshot_session(&save_app, &save_snapshot, &save_session_id)?;
        Ok(save_snapshot)
    })
    .await?;

    Ok(BuiltinCommandRunResult {
        command_id: "rename".to_owned(),
        notice: format!("已将会话重命名为「{title}」。"),
        snapshot: Some(saved),
    })
}

/** 整理当前会话上下文；复用与 `compact_agent_context` 相同的 runtime 摘要能力。 */
async fn compact_session_from_command(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<BuiltinCommandRunResult, String> {
    let session_id = snapshot.active_session_id.clone();
    if runtime::is_session_turn_active(&session_id) {
        return Err("当前会话仍有进行中的回合，请先停止后再整理上下文。".to_owned());
    }

    let message_count = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .map(|session| session.messages.len())
        .unwrap_or(0);
    if message_count == 0 {
        return Err("当前会话没有可整理的对话消息。".to_owned());
    }

    let settings_app = app.clone();
    let settings = run_blocking("读取模型设置", move || {
        storage::load_user_settings(&settings_app)
    })
    .await?;
    let compacted =
        runtime::compact_agent_context_summary(app, snapshot, &session_id, settings).await?;
    let persist_app = app.clone();
    let persist_snapshot = compacted.clone();
    let persist_session_id = session_id.clone();
    run_blocking("保存会话上下文", move || {
        storage::save_snapshot_session(&persist_app, &persist_snapshot, &persist_session_id)
    })
    .await?;

    Ok(BuiltinCommandRunResult {
        command_id: "compact".to_owned(),
        notice: format!("已整理当前会话上下文（{message_count} 条消息）。"),
        snapshot: Some(compacted),
    })
}

#[cfg(test)]
mod tests {
    use super::build_new_command_session;
    use crate::storage::DEFAULT_SESSION_TITLE;

    /**
     * `/new` 必须写入占位标题，而不是空串。
     *
     * 标题补齐逻辑（`agent.rs` / `runtime/mod.rs`）只认占位标题 + 无用户消息这两个条件；
     * 一旦这里写成空串，会话标题就再也不会被首条输入接管，表现为「新会话没有标题」。
     */
    #[test]
    fn new_command_uses_placeholder_title_so_first_prompt_can_take_over() {
        let session = build_new_command_session("kb-a");
        let has_user_message = session
            .messages
            .iter()
            .any(|message| message.role == "user");

        assert_eq!(session.title, DEFAULT_SESSION_TITLE);
        assert!(!has_user_message);
        assert_eq!(session.knowledge_base_ids, vec!["kb-a".to_owned()]);
        // 用户没自定义过标题，后续才允许被首条输入替换。
        assert!(!session.title_customized);
    }
}
