//! 发给模型的上下文投影：唯一 system、检查点 user、本轮 user、合法切点。
//! 不调模型、不写 SQLite、不跑工具。

use crate::agent_trace::is_user_visible_tool;
use crate::domain::{
    AgentContextSummary, AgentMemoryEntry, AgentMessage, AgentPromptDump, AgentPromptDumpMessage,
    AgentSession, AgentSkill, AgentToolCall, AgentTurnRequest, KnowledgeBaseMemory, ProposedChange,
    WorkspaceSnapshot, MEMORY_CATEGORY_CONVENTION, MEMORY_CATEGORY_NOTE_STRUCTURE,
    MEMORY_CATEGORY_ORGANIZATION, MEMORY_CATEGORY_OTHER, MEMORY_CATEGORY_TAG_CONVENTION,
};
use crate::model_provider;
use crate::skills;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/** 未知模型窗口时按 256k tokens 估算。 */
pub(super) const DEFAULT_MODEL_CONTEXT_TOKENS: u64 = 256_000;

/** 会话历史最多占用模型窗口的比例。 */
pub(super) const HISTORY_CONTEXT_WINDOW_RATIO: f64 = 0.40;

/** 混合中英按 2 字符/token 估算。 */
pub(super) const CHARS_PER_TOKEN_ESTIMATE: u64 = 2;

/** 未知窗口时的历史层下限。 */
pub(super) const MIN_HISTORY_BUDGET_CHARS: usize = 16_000;

/** 历史层上限，对齐 256k 保守窗口的 40% 份额。 */
pub(super) const MAX_HISTORY_BUDGET_CHARS: usize = 204_800;

/** 已知小窗口时允许的历史层下限。 */
pub(super) const MIN_KNOWN_HISTORY_BUDGET_CHARS: usize = 4_000;

/** 安全阀：最多回放多少条会话消息。 */
const MAX_MODEL_HISTORY_SESSION_MESSAGES: usize = 80;

/** 最近若干条会话消息视为热窗口。 */
pub(super) const HOT_HISTORY_SESSION_MESSAGES: usize = 16;

/** 即使超预算也至少保留最近几条会话消息。 */
pub(super) const MIN_RETAINED_SESSION_MESSAGES: usize = 2;

const MAX_HOT_HISTORY_MESSAGE_CHARS: usize = 8000;
const MAX_WARM_HISTORY_MESSAGE_CHARS: usize = 2500;
pub(super) const MAX_HISTORY_MESSAGE_CHARS: usize = MAX_HOT_HISTORY_MESSAGE_CHARS;
const MAX_HOT_HISTORY_TOOL_RESULT_CHARS: usize = 4000;
const MAX_HISTORY_TOOL_ARGUMENT_STRING_CHARS: usize = 360;
const MAX_HOT_HISTORY_TOOL_ARGUMENT_STRING_CHARS: usize = 1200;
pub(super) const MAX_RENDERED_CONTEXT_SUMMARY_CHARS: usize = 6000;
pub(super) const MAX_CONTEXT_SUMMARY_ITEM_CHARS: usize = 360;
pub(super) const MAX_CONTEXT_SUMMARY_ITEMS: usize = 12;
const MAX_PROJECT_AGENT_INSTRUCTION_CHARS: usize = 16 * 1024;
pub(super) const MAX_RENDERED_KB_MEMORY_CHARS: usize = 4000;
const MAX_RENDERED_KB_MEMORY_ENTRIES_PER_KB: usize = 8;
const MAX_MENTIONED_FILES_PER_TURN: usize = 8;
const MAX_MENTIONED_TEXT_CHARS: usize = 12_000;

/** 回复和摘要预留：窗口的 20% 与 16k token 估算字符的较大者。 */
pub(super) const COMPACT_RESERVE_WINDOW_RATIO: f64 = 0.20;
pub(super) const COMPACT_RESERVE_MIN_TOKENS: u64 = 16_000;

/** 会话历史按窗口预算装箱后的统计。 */
#[derive(Clone, Debug, Default)]
pub(super) struct PackedHistoryStats {
    pub included_session_messages: usize,
    pub dropped_session_messages: usize,
    pub budget_chars: usize,
    pub used_chars: usize,
}

/** 发给模型的完整 prompt。prefix_len 始终为 1。 */
pub(super) struct ModelPrompt {
    pub messages: Vec<Value>,
    pub history: PackedHistoryStats,
    pub prefix_len: usize,
}

struct PackedHistory {
    messages: Vec<Value>,
    stats: PackedHistoryStats,
}

/** 已通过 scope 校验的本轮显式材料。 */
pub(super) struct MentionedFileMaterial {
    pub id: String,
    pub knowledge_base_id: String,
    pub title: String,
    pub path: String,
    pub file_type: String,
    pub content: Option<String>,
    pub image_markdown_path: Option<String>,
}

/** 构造模型可用的 system、检查点和历史消息；测试默认按未知窗口装箱。 */
#[cfg(test)]
pub(super) fn build_model_messages(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    current_user_message_id: &str,
    knowledge_base_memories: &[KnowledgeBaseMemory],
) -> Vec<Value> {
    build_model_prompt(
        snapshot,
        session_index,
        request,
        available_skills,
        explicit_skills,
        current_user_message_id,
        knowledge_base_memories,
        None,
        None,
    )
    .messages
}

/** 按四层投影会话：唯一 system + 检查点 user + 历史 + 本轮 user。 */
pub(super) fn build_model_prompt(
    snapshot: &WorkspaceSnapshot,
    session_index: usize,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    explicit_skills: &[AgentSkill],
    current_user_message_id: &str,
    knowledge_base_memories: &[KnowledgeBaseMemory],
    model_context_length: Option<u64>,
    previous_transcript: Option<&[Value]>,
) -> ModelPrompt {
    let session = &snapshot.sessions[session_index];
    let system_content = build_system_prompt(
        snapshot,
        session,
        request,
        available_skills,
        knowledge_base_memories,
    );
    let mut messages = vec![json!({
        "role": "system",
        "content": system_content
    })];
    let prefix_len = 1;
    let mentioned_files_prompt =
        render_mentioned_files_prompt(&resolve_mentioned_files(snapshot, session, request));
    let pending_prompt = render_pending_live_prompt(session);
    // 工作记忆每轮都会更新给 UI 看；检查点只在历史已经装不下时才进模型上下文。
    let checkpoint = if should_inject_checkpoint(session, model_context_length) {
        render_checkpoint_user_message(session.context_summary.as_ref())
    } else {
        None
    };
    let context_summary_prompt_chars = checkpoint
        .as_ref()
        .and_then(|message| message.get("content").and_then(Value::as_str))
        .map(|content| content.chars().count())
        .unwrap_or_default();
    let context_summary_updated_at = session
        .context_summary
        .as_ref()
        .map(|summary| summary.updated_at.as_str())
        .filter(|updated_at| !updated_at.trim().is_empty())
        .unwrap_or("none");
    let knowledge_base_memory_chars =
        render_knowledge_base_memory_prompt(knowledge_base_memories, &snapshot.knowledge_bases)
            .map(|prompt| prompt.chars().count())
            .unwrap_or_default();

    let history = if let Some(transcript) = previous_transcript
        .filter(|messages| !messages.is_empty() && transcript_is_usable(messages, session))
    {
        let mut conversation = project_transcript(transcript, session.context_summary.as_ref());
        if conversation_starts_with_checkpoint(&conversation) && checkpoint.is_none() {
            conversation = strip_existing_checkpoint(&conversation).to_vec();
        } else if !conversation_starts_with_checkpoint(&conversation) {
            if let Some(checkpoint) = checkpoint {
                conversation.insert(0, checkpoint);
            }
        }
        messages.extend(conversation);
        messages.push(build_current_user_model_message(
            request,
            explicit_skills,
            mentioned_files_prompt.as_deref(),
            pending_prompt.as_deref(),
        ));
        let used_chars = estimate_model_messages_chars(&messages[prefix_len..]);
        PackedHistoryStats {
            included_session_messages: transcript.len().saturating_add(1),
            dropped_session_messages: 0,
            budget_chars: resolve_history_budget_chars(model_context_length),
            used_chars,
        }
    } else {
        if let Some(checkpoint) = checkpoint {
            messages.push(checkpoint);
        }
        let packed_history = session_history_model_messages(
            session,
            request,
            current_user_message_id,
            model_context_length,
        );
        messages.extend(packed_history.messages);
        attach_turn_materials_to_current_user(
            &mut messages,
            prefix_len,
            request,
            explicit_skills,
            mentioned_files_prompt.as_deref(),
            pending_prompt.as_deref(),
        );
        packed_history.stats
    };

    log::debug!(
        target: "agent_runtime",
        "上下文注入完成：session={} transcript_mode={} summary_injected={} summary_chars={} summary_updated_at={} has_pending_change={} project_instruction_count={} kb_memory_injected={} kb_memory_chars={} kb_memory_entry_count={} kb_memory_kb_count={} history_included={} history_dropped={} history_budget_chars={} history_used_chars={} model_context_length={}",
        session.id,
        if previous_transcript.is_some_and(|messages| !messages.is_empty()) {
            "append"
        } else {
            "seed"
        },
        context_summary_prompt_chars > 0,
        context_summary_prompt_chars,
        context_summary_updated_at,
        session.pending_change.as_ref().is_some_and(|change| change.status == "pending"),
        project_instruction_count(snapshot, session),
        knowledge_base_memory_chars > 0,
        knowledge_base_memory_chars,
        knowledge_base_memories.iter().map(|memory| memory.entries.len()).sum::<usize>(),
        knowledge_base_memories.len(),
        history.included_session_messages,
        history.dropped_session_messages,
        history.budget_chars,
        history.used_chars,
        model_context_length.unwrap_or(0)
    );

    ModelPrompt {
        messages,
        history,
        prefix_len,
    }
}

/** 从完整模型请求中切出可持久化的对话 transcript，丢掉唯一 system 前缀。 */
pub(super) fn conversation_from_model_messages(
    messages: &[Value],
    prefix_len: usize,
) -> Vec<Value> {
    messages.get(prefix_len..).unwrap_or(&[]).to_vec()
}

/** 拼唯一 system：角色、工具、guideline、安全级别、项目指令、Skill 目录、跨会话记忆、范围。 */
fn build_system_prompt(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
    request: &AgentTurnRequest,
    available_skills: &[AgentSkill],
    knowledge_base_memories: &[KnowledgeBaseMemory],
) -> String {
    let visible_tools = if session.im_identity.is_some() || session.security_level == "basic" {
        "search, read, list, edit, write"
    } else {
        "search, read, list, edit, write, run"
    };
    let write_policy = if session.security_level == "autonomous" {
        "所有写入只能调用 edit 或 write；校验通过后会自动落盘，不要在工具结果返回前声称已经写入文件。"
    } else {
        "所有写入只能调用 edit 或 write 生成待确认 diff，不能声称已经写入文件。"
    };
    let security_level_policy = match session.security_level.as_str() {
        "autonomous" => "当前为完全级别：用户把连续执行权交给你，目的是让你一次把任务做完。edit 和 write（含建文件夹）在校验通过后会自动落盘；失败则保留待确认。你可以在当前 scope 内知识库使用相对路径，也可以对用户设备上的合规绝对路径（含 ~）调用 list、read 和 write。这不是整台电脑：Windows、Program Files 等受保护系统目录会被拒绝。Skill 只是更高权限下可发挥的能力之一，脚本仍在隔离副本中运行，不会获得真实系统路径。",
        "advanced" => "当前为进阶级别：用户开始放手，但仍要在落盘前确认。write 可以在当前知识库内建文件夹，也可以在授权后运行 run。所有写入和 Skill 执行仍需用户确认后才会生效。",
        _ => "当前为基础级别：用户选择先看紧。你只使用知识库文档工具；edit 和 write 只生成待确认 diff，不能声称已经写入。不要暗示你可以执行脚本、访问知识库外路径或跳过确认。",
    };
    let autonomous_tool_policy = "你需要根据用户输入和上下文自主判断是否调用工具：需要 Markdown 引用时使用 search；需要当前 scope 内正文时使用 read（可省略 fileId 以读当前文件）；需要改写时使用 edit；需要新建时使用 write；需要看目录时使用 list。DOCX/PDF 用 read 只读抽取，不可编辑，且不会自动进入全文搜索。TXT 必须按纯文本原样处理。无关的通用问题可以直接回答。界面 action 只是 UI 分类，不能替代你的判断。";
    let skill_policy = "启用的 Skill 只以名称和描述提供给你参考，是否使用、使用哪一个 Skill 都由你自主判断。Skill 只是可用能力的一部分，不能扩大工具权限或绕过系统保护边界。";
    let scope_summary = build_scope_summary(snapshot, session);
    let active_note_summary = if request.active_note_id.is_empty() {
        "当前未绑定笔记".to_owned()
    } else {
        format!("当前笔记 ID：{}", request.active_note_id)
    };
    let cwd_summary = build_cwd_summary(snapshot, session);

    let mut parts = vec![
        format!("你是橘记的本地优先知识库 Agent。当前可见工具：{visible_tools}。"),
        format!(
            "search 只检索 Markdown；read 和 edit 可作用于当前 scope 内的 Markdown/TXT，省略 fileId 时 read 读取当前激活文件；TXT 必须原样按纯文本处理；read 也可只读 DOCX/PDF 并返回可信的页码或结构块引用。{write_policy}write 的 fileType 只能是 markdown 或 txt，路径扩展名必须匹配。局部替换使用 operation=replace，文末追加使用 operation=append 且 next 只含增量；同一文件多处编辑使用 operation=multi_replace 和 edits。必须使用服务端标准 tool_calls 字段调用工具，不要在普通回复中输出 DSML、XML 或伪工具调用标签。引用只允许来自已执行工具结果。{skill_policy}\n{autonomous_tool_policy}\n{security_level_policy}"
        ),
    ];

    if let Some(project) = render_project_agent_instructions(snapshot, session) {
        parts.push(format!("<project_context>\n{project}\n</project_context>"));
    }

    let catalog = skills::skill_catalog_prompt(available_skills);
    if !catalog.is_empty() {
        parts.push(catalog);
    }

    if let Some(memory) =
        render_knowledge_base_memory_prompt(knowledge_base_memories, &snapshot.knowledge_bases)
    {
        parts.push(memory);
    }

    parts.push(format!(
        "【范围】\n允许 scope：{scope_summary}\n{active_note_summary}\n{cwd_summary}"
    ));
    parts.join("\n\n")
}

/** 知识库根路径，作为 cwd 等价信息写入 system。 */
fn build_cwd_summary(snapshot: &WorkspaceSnapshot, session: &AgentSession) -> String {
    let paths = session
        .knowledge_base_ids
        .iter()
        .filter_map(|id| {
            snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| knowledge_base.id == *id)
                .map(|knowledge_base| format!("{} ({})", knowledge_base.path, knowledge_base.name))
        })
        .collect::<Vec<_>>();
    if paths.is_empty() {
        "cwd：未绑定知识库根目录".to_owned()
    } else {
        format!("cwd / 知识库根路径：{}", paths.join(" / "))
    }
}

/** 汇总会话允许的知识库名称，用于 system prompt 和请求审计。 */
pub(super) fn build_scope_summary(snapshot: &WorkspaceSnapshot, session: &AgentSession) -> String {
    let labels = session
        .knowledge_base_ids
        .iter()
        .filter_map(|id| {
            snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| knowledge_base.id == *id)
                .map(|knowledge_base| format!("{} (id={})", knowledge_base.name, knowledge_base.id))
        })
        .collect::<Vec<_>>();

    if labels.is_empty() {
        "未绑定知识库".to_owned()
    } else if labels.len() == 1 {
        labels[0].clone()
    } else {
        format!("{} 个知识库：{}", labels.len(), labels.join(" / "))
    }
}

/** 构造本轮发给模型的 user 消息：action、slash Skill、用户原话、@、pending。 */
pub(super) fn build_current_user_model_message(
    request: &AgentTurnRequest,
    explicit_skills: &[AgentSkill],
    mentioned_files_prompt: Option<&str>,
    pending_prompt: Option<&str>,
) -> Value {
    let mut content = format!("界面 action 提示：{}", request.action);
    let skill_prompt = skills::explicit_skill_prompt(explicit_skills);
    if !skill_prompt.is_empty() {
        content.push_str("\n\n");
        content.push_str(&skill_prompt);
    }
    content.push_str(&format!("\n用户输入：{}", request.prompt));
    if let Some(mentioned) = mentioned_files_prompt.filter(|value| !value.is_empty()) {
        content.push_str("\n\n");
        content.push_str(mentioned);
    }
    if let Some(pending) = pending_prompt.filter(|value| !value.is_empty()) {
        content.push_str("\n\n");
        content.push_str(pending);
    }
    json!({
        "role": "user",
        "content": content
    })
}

/** seed 路径把本轮 Skill/@/pending 并进当前 user，避免再写成每轮重建的 system。 */
fn attach_turn_materials_to_current_user(
    messages: &mut Vec<Value>,
    prefix_len: usize,
    request: &AgentTurnRequest,
    explicit_skills: &[AgentSkill],
    mentioned_files_prompt: Option<&str>,
    pending_prompt: Option<&str>,
) {
    let current = build_current_user_model_message(
        request,
        explicit_skills,
        mentioned_files_prompt,
        pending_prompt,
    );
    if let Some(user_message) = messages
        .iter_mut()
        .skip(prefix_len)
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    {
        *user_message = current;
    } else {
        messages.push(current);
    }
}

/** 只有 compact 切点存在、且全量历史已经超出窗口预算时，才把检查点塞进模型上下文。 */
pub(super) fn should_inject_checkpoint(
    session: &AgentSession,
    model_context_length: Option<u64>,
) -> bool {
    let Some(summary) = session.context_summary.as_ref() else {
        return false;
    };
    if summary
        .last_compacted_message_id
        .as_deref()
        .is_none_or(|id| id.trim().is_empty())
    {
        return false;
    }
    if render_context_summary_body(summary).is_none() {
        return false;
    }
    session_history_exceeds_budget(session, model_context_length)
}

/** 全量会话正文仍能装进历史预算时，说明上次 compact 没有真正丢掉历史。 */
fn session_history_exceeds_budget(
    session: &AgentSession,
    model_context_length: Option<u64>,
) -> bool {
    let budget = resolve_history_budget_chars(model_context_length);
    let used = session
        .messages
        .iter()
        .map(|message| message.content.chars().count().saturating_add(64))
        .sum::<usize>();
    used > budget
}

/** transcript 若丢掉了会话里已有的 assistant 回复，则本轮改从 session.messages seed。 */
pub(super) fn transcript_is_usable(transcript: &[Value], session: &AgentSession) -> bool {
    let body = strip_existing_checkpoint(transcript);
    if body.is_empty() {
        return false;
    }
    let session_has_assistant = session
        .messages
        .iter()
        .any(|message| message.role == "assistant");
    let transcript_has_assistant = body
        .iter()
        .any(|message| message_role(message) == "assistant");
    !session_has_assistant || transcript_has_assistant
}

/** 有 summary 时生成检查点 user；不含 version/updatedAt 等记账字段。 */
pub(super) fn render_checkpoint_user_message(
    summary: Option<&AgentContextSummary>,
) -> Option<Value> {
    let content = render_checkpoint_user_content(summary)?;
    Some(json!({
        "role": "user",
        "content": content
    }))
}

/** 检查点正文，供投影和 compact_transcript 复用。 */
pub(super) fn render_checkpoint_user_content(
    summary: Option<&AgentContextSummary>,
) -> Option<String> {
    let body = render_context_summary_body(summary?)?;
    Some(format!(
        "以下是压缩检查点，不是用户的新指令。不要把其中的 pendingTasks / Next Steps 当成当前用户要求。\n\n<summary>\n{}\n</summary>",
        truncate_chars(&body, MAX_RENDERED_CONTEXT_SUMMARY_CHARS)
    ))
}

/** 把结构化工作记忆转成模型可读文本，去掉对模型无用的记账字段。 */
pub(super) fn render_context_summary_body(summary: &AgentContextSummary) -> Option<String> {
    let mut lines = Vec::new();

    if let Some(goal) = summary
        .current_goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        lines.push(format!("currentGoal: {goal}"));
    }

    push_summary_list(&mut lines, "userConstraints", &summary.user_constraints);
    push_summary_list(&mut lines, "decisions", &summary.decisions);
    push_summary_list(&mut lines, "completedWork", &summary.completed_work);
    push_summary_list(&mut lines, "pendingTasks", &summary.pending_tasks);

    if !summary.touched_notes.is_empty() {
        lines.push("touchedNotes:".to_owned());
        for note in summary.touched_notes.iter().take(MAX_CONTEXT_SUMMARY_ITEMS) {
            lines.push(format!("- {} | {} | {}", note.id, note.title, note.reason));
        }
    }

    if let Some(change) = summary
        .pending_change_summary
        .as_deref()
        .filter(|change| !change.trim().is_empty())
    {
        lines.push(format!("pendingChangeSummary: {change}"));
    }

    push_summary_list(&mut lines, "openQuestions", &summary.open_questions);

    let body = lines.join("\n");
    (!body.trim().is_empty()).then_some(body)
}

/** 只计算工作记忆渲染长度，供日志和审计记录使用。 */
pub(super) fn context_summary_rendered_chars(summary: Option<&AgentContextSummary>) -> usize {
    summary
        .and_then(render_context_summary_body)
        .map(|body| body.chars().count())
        .unwrap_or_default()
}

fn push_summary_list(lines: &mut Vec<String>, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    for item in items.iter().take(MAX_CONTEXT_SUMMARY_ITEMS) {
        lines.push(format!("- {item}"));
    }
}

/** 活 pending 状态挂在本轮 user，确认/拒绝后下一轮自然消失。 */
pub(super) fn render_pending_live_prompt(session: &AgentSession) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(change) = render_pending_change_prompt(session.pending_change.as_ref()) {
        sections.push(change);
    }
    if let Some(change_set) = session
        .pending_change_set
        .as_ref()
        .filter(|change_set| change_set.status == "pending")
    {
        sections.push(format!(
            "【当前待确认变更集】\n以下变更集尚未落盘，不要当成已经写入文件。\n- skillId={}\n- operations={}\n- summary={}",
            change_set.skill_id,
            change_set.operations.len(),
            change_set.summary
        ));
    }
    if let Some(execution) = session
        .pending_execution
        .as_ref()
        .filter(|execution| execution.status == "pending")
    {
        sections.push(format!(
            "【当前待确认 Skill 执行】\n以下执行尚未落盘。\n- skill={}\n- command={}",
            execution.skill_name, execution.command_preview
        ));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

/** 渲染待确认 diff 摘要，只暴露状态和统计。 */
pub(super) fn render_pending_change_prompt(change: Option<&ProposedChange>) -> Option<String> {
    let change = change?;
    if change.status != "pending" {
        return None;
    }
    let summary = summarize_pending_change(change)?;
    Some(format!(
        "【当前待确认变更】\n以下是当前会话的 diff 状态摘要。不要把它当成已写入文件；只有用户确认后才会落盘。\n{summary}"
    ))
}

/** 生成 pending change 的脱敏摘要。 */
pub(super) fn summarize_pending_change(change: &ProposedChange) -> Option<String> {
    if change.status.trim().is_empty() {
        return None;
    }
    let operation = change.operation.as_deref().unwrap_or("create");
    let stats = change.diff_stats.as_ref().map(|stats| {
        format!(
            "addedLines={} removedLines={} hunkCount={} originalChars={} nextChars={}",
            stats.added_lines,
            stats.removed_lines,
            stats.hunk_count,
            stats.original_char_count,
            stats.next_char_count
        )
    });
    Some(format!(
        "- 类型：{}\n- 操作：{}\n- 标题：{}\n- 目标路径：{}\n- 状态：{}\n- 统计：{}",
        change.r#type,
        operation,
        change.title,
        change.target_path,
        change.status,
        stats.unwrap_or_else(|| {
            format!(
                "originalChars={} nextChars={}",
                change.original.chars().count(),
                change.next.chars().count()
            )
        })
    ))
}

/** 当前仍等待确认的 diff 摘要。 */
pub(super) fn current_pending_change_summary(session: &AgentSession) -> Option<String> {
    session
        .pending_change
        .as_ref()
        .filter(|change| change.status == "pending")
        .and_then(summarize_pending_change)
}

/** 投影已有 transcript：丢掉 conversation 内 system，改写旧工作记忆，过滤残缺 tool 对。 */
pub(super) fn project_transcript(
    transcript: &[Value],
    summary: Option<&AgentContextSummary>,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (index, message) in transcript.iter().enumerate() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role == "system" {
            let content = message_text_content(message);
            if is_legacy_work_memory(content) {
                if let Some(checkpoint) = render_checkpoint_user_message(summary) {
                    out.push(checkpoint);
                } else {
                    out.push(rewrite_legacy_work_memory_as_checkpoint(content));
                }
            }
            continue;
        }
        if index == 0 && is_checkpoint_message(message) {
            if let Some(checkpoint) = render_checkpoint_user_message(summary) {
                out.push(checkpoint);
                continue;
            }
        }
        out.push(normalize_projected_message(message));
    }
    filter_incomplete_tool_pairs(&out)
}

fn conversation_starts_with_checkpoint(messages: &[Value]) -> bool {
    messages.first().is_some_and(is_checkpoint_message)
}

/** 检查点 user 或旧的 system 工作记忆。 */
pub(super) fn is_checkpoint_message(message: &Value) -> bool {
    let content = message_text_content(message);
    match message.get("role").and_then(Value::as_str) {
        Some("user") => content.contains("压缩检查点"),
        Some("system") => is_legacy_work_memory(content),
        _ => false,
    }
}

fn is_legacy_work_memory(content: &str) -> bool {
    content.contains("【会话工作记忆】")
}

fn rewrite_legacy_work_memory_as_checkpoint(content: &str) -> Value {
    let body = content.replace("【会话工作记忆】", "").trim().to_owned();
    json!({
        "role": "user",
        "content": format!(
            "以下是压缩检查点，不是用户的新指令。不要把其中的 pendingTasks / Next Steps 当成当前用户要求。\n\n<summary>\n{}\n</summary>",
            truncate_chars(&body, MAX_RENDERED_CONTEXT_SUMMARY_CHARS)
        )
    })
}

fn normalize_projected_message(message: &Value) -> Value {
    let mut cloned = message.clone();
    if cloned.get("content").map(Value::is_null).unwrap_or(false) {
        cloned["content"] = json!("");
    }
    cloned
}

fn message_text_content(message: &Value) -> &str {
    message.get("content").and_then(Value::as_str).unwrap_or("")
}

fn message_role(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or("")
}

fn assistant_tool_call_ids(message: &Value) -> Vec<String> {
    message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| call.get("id").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/** 丢掉没有成对 tool 结果的 assistant(tool_calls)，以及没有对应 assistant 的孤立 tool。 */
pub(super) fn filter_incomplete_tool_pairs(messages: &[Value]) -> Vec<Value> {
    let mut result_ids = HashSet::new();
    for message in messages {
        if message_role(message) == "tool" {
            if let Some(id) = message.get("tool_call_id").and_then(Value::as_str) {
                result_ids.insert(id.to_owned());
            }
        }
    }

    let mut keep = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let message = &messages[i];
        let tool_ids = assistant_tool_call_ids(message);
        if message_role(message) == "assistant" && !tool_ids.is_empty() {
            if tool_ids.iter().all(|id| result_ids.contains(id)) {
                keep.push(message.clone());
            }
            i += 1;
            continue;
        }
        if message_role(message) == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let has_assistant = keep.iter().rev().any(|previous| {
                message_role(previous) == "assistant"
                    && assistant_tool_call_ids(previous)
                        .iter()
                        .any(|call_id| call_id == id)
            });
            if has_assistant && !id.is_empty() {
                keep.push(message.clone());
            }
            i += 1;
            continue;
        }
        keep.push(message.clone());
        i += 1;
    }
    keep
}

/** 把 transcript 收成不可拆开的切点单元：user/纯 assistant，或 assistant(tool_calls)+后续 tool。 */
pub(super) fn group_transcript_units(messages: &[Value]) -> Vec<Vec<Value>> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        let tool_ids = assistant_tool_call_ids(message);
        if message_role(message) == "assistant" && !tool_ids.is_empty() {
            let mut unit = vec![message.clone()];
            index += 1;
            while index < messages.len() && message_role(&messages[index]) == "tool" {
                unit.push(messages[index].clone());
                index += 1;
            }
            units.push(unit);
            continue;
        }
        units.push(vec![message.clone()]);
        index += 1;
    }
    units
}

/** 去掉已有检查点后再分组，避免 compact 把旧检查点算进 tail。 */
pub(super) fn strip_existing_checkpoint(transcript: &[Value]) -> &[Value] {
    match transcript.first() {
        Some(message) if is_checkpoint_message(message) => transcript.get(1..).unwrap_or(&[]),
        _ => transcript,
    }
}

/** 按模型窗口从新到旧装箱会话历史；有 last_compacted_message_id 时不扫该 id 及之前。 */
fn session_history_model_messages(
    session: &AgentSession,
    request: &AgentTurnRequest,
    current_user_message_id: &str,
    model_context_length: Option<u64>,
) -> PackedHistory {
    let budget_chars = resolve_history_budget_chars(model_context_length);
    let start_index = if should_inject_checkpoint(session, model_context_length) {
        session
            .context_summary
            .as_ref()
            .and_then(|summary| summary.last_compacted_message_id.as_deref())
            .and_then(|id| {
                session
                    .messages
                    .iter()
                    .position(|message| message.id == id)
                    .map(|index| index + 1)
            })
            .unwrap_or(0)
    } else {
        0
    };
    let eligible = session.messages.get(start_index..).unwrap_or(&[]);
    let candidates = eligible
        .iter()
        .rev()
        .take(MAX_MODEL_HISTORY_SESSION_MESSAGES)
        .collect::<Vec<_>>();
    let dropped_by_cap = eligible.len().saturating_sub(candidates.len());

    let mut groups = Vec::new();
    let mut used_chars = 0usize;

    for (offset, message) in candidates.iter().enumerate() {
        let is_hot = offset < HOT_HISTORY_SESSION_MESSAGES;
        let group = history_messages_from_session_message(
            message,
            request,
            current_user_message_id,
            is_hot,
        );
        let group_chars = estimate_model_messages_chars(&group);
        let must_keep = offset < MIN_RETAINED_SESSION_MESSAGES;
        if !must_keep && used_chars + group_chars > budget_chars {
            break;
        }
        used_chars = used_chars.saturating_add(group_chars);
        groups.push(group);
    }

    let included_session_messages = groups.len();
    let dropped_session_messages =
        dropped_by_cap + candidates.len().saturating_sub(included_session_messages);
    groups.reverse();

    PackedHistory {
        messages: groups.into_iter().flatten().collect(),
        stats: PackedHistoryStats {
            included_session_messages,
            dropped_session_messages,
            budget_chars,
            used_chars,
        },
    }
}

/** 根据模型 context_length 计算历史层字符预算。 */
pub(super) fn resolve_history_budget_chars(model_context_length: Option<u64>) -> usize {
    let Some(tokens) = model_context_length.filter(|tokens| *tokens >= 1_024) else {
        let default_chars = (DEFAULT_MODEL_CONTEXT_TOKENS.saturating_mul(CHARS_PER_TOKEN_ESTIMATE)
            as f64
            * HISTORY_CONTEXT_WINDOW_RATIO) as usize;
        return default_chars.clamp(MIN_HISTORY_BUDGET_CHARS, MAX_HISTORY_BUDGET_CHARS);
    };
    let chars = (tokens.saturating_mul(CHARS_PER_TOKEN_ESTIMATE) as f64
        * HISTORY_CONTEXT_WINDOW_RATIO) as usize;
    chars.clamp(MIN_KNOWN_HISTORY_BUDGET_CHARS, MAX_HISTORY_BUDGET_CHARS)
}

/** 为回复和摘要预留的字符数。 */
pub(super) fn resolve_compact_reserve_chars(model_context_length: Option<u64>) -> usize {
    let window_tokens = model_context_length
        .filter(|tokens| *tokens >= 1_024)
        .unwrap_or(DEFAULT_MODEL_CONTEXT_TOKENS);
    let ratio_tokens = (window_tokens as f64 * COMPACT_RESERVE_WINDOW_RATIO) as u64;
    let reserve_tokens = ratio_tokens.max(COMPACT_RESERVE_MIN_TOKENS);
    reserve_tokens.saturating_mul(CHARS_PER_TOKEN_ESTIMATE) as usize
}

/** 单条会话消息转成模型协议消息；基建工具不会伪装成模型 tool_calls。 */
fn history_messages_from_session_message(
    message: &AgentMessage,
    request: &AgentTurnRequest,
    current_user_message_id: &str,
    is_hot: bool,
) -> Vec<Value> {
    let max_content_chars = if is_hot {
        MAX_HOT_HISTORY_MESSAGE_CHARS
    } else {
        MAX_WARM_HISTORY_MESSAGE_CHARS
    };

    if message.role == "user" {
        let content = if message.id == current_user_message_id {
            format!(
                "界面 action 提示：{}\n用户输入：{}",
                request.action, message.content
            )
        } else {
            message.content.clone()
        };
        return vec![json!({
            "role": "user",
            "content": truncate_chars(&content, max_content_chars)
        })];
    }

    let replayable_tools = message
        .tool_calls
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|tool_call| is_user_visible_tool(&tool_call.name))
        .collect::<Vec<_>>();

    if replayable_tools.is_empty() {
        return vec![json!({
            "role": message.role,
            "content": truncate_chars(&message.content, max_content_chars)
        })];
    }

    let assistant_content = if message.content.trim().is_empty() {
        json!("")
    } else {
        json!(truncate_chars(&message.content, max_content_chars))
    };
    let tool_previews = message
        .trace
        .iter()
        .filter(|step| step.step_type == "tool")
        .map(|step| step.result_preview.as_deref())
        .collect::<Vec<_>>();
    let mut history = vec![json!({
        "role": "assistant",
        "content": assistant_content,
        "tool_calls": replayable_tools
            .iter()
            .map(|tool_call| history_tool_call_payload(tool_call, is_hot))
            .collect::<Vec<_>>()
    })];

    for (index, tool_call) in replayable_tools.iter().enumerate() {
        history.push(json!({
            "role": "tool",
            "tool_call_id": tool_call.id,
            "content": history_tool_result_content(
                tool_call,
                tool_previews.get(index).copied().flatten(),
                is_hot
            )
        }));
    }
    history
}

fn history_tool_call_payload(tool_call: &AgentToolCall, is_hot: bool) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": history_tool_arguments(&tool_call.args, is_hot)
        }
    })
}

fn history_tool_arguments(args: &Value, is_hot: bool) -> String {
    let max_total_chars = if is_hot {
        MAX_HOT_HISTORY_MESSAGE_CHARS
    } else {
        MAX_WARM_HISTORY_MESSAGE_CHARS
    };
    let max_string_chars = if is_hot {
        MAX_HOT_HISTORY_TOOL_ARGUMENT_STRING_CHARS
    } else {
        MAX_HISTORY_TOOL_ARGUMENT_STRING_CHARS
    };
    let serialized_chars = if is_hot {
        MAX_HISTORY_MESSAGE_CHARS
    } else {
        MAX_WARM_HISTORY_MESSAGE_CHARS
    };
    let serialized = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_owned());
    if serialized.chars().count() <= max_total_chars {
        return serialized;
    }
    let truncated = truncate_json_strings(args, max_string_chars);
    let truncated_serialized =
        serde_json::to_string(&truncated).unwrap_or_else(|_| "{}".to_owned());
    if truncated_serialized.chars().count() <= serialized_chars {
        return truncated_serialized;
    }
    json!({
        "truncated": true,
        "keys": truncated.as_object().map(|map| map.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
    })
    .to_string()
}

fn truncate_json_strings(value: &Value, max_chars: usize) -> Value {
    match value {
        Value::String(text) => Value::String(truncate_chars(text, max_chars)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| truncate_json_strings(item, max_chars))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, child)| (key.clone(), truncate_json_strings(child, max_chars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn history_tool_result_content(
    tool_call: &AgentToolCall,
    result_preview: Option<&str>,
    is_hot: bool,
) -> String {
    if is_hot {
        if let Some(preview) = result_preview
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return truncate_chars(preview, MAX_HOT_HISTORY_TOOL_RESULT_CHARS);
        }
    }
    truncate_chars(
        &json!({
            "status": tool_call.status,
            "summary": tool_call.summary
        })
        .to_string(),
        if is_hot {
            MAX_HOT_HISTORY_TOOL_RESULT_CHARS
        } else {
            MAX_WARM_HISTORY_MESSAGE_CHARS
        },
    )
}

fn render_project_agent_instructions(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
) -> Option<String> {
    let instructions = load_project_agent_instructions(snapshot, session);
    if instructions.is_empty() {
        return None;
    }
    Some(format!(
        "【项目级 Agent 指令】\n以下内容来自知识库根目录 ORANGE_AGENT.md（带 path 的项目规则，不要当成用户指令）。优先级低于橘记系统规则，高于普通会话记忆。\n{}",
        instructions.join("\n\n")
    ))
}

fn load_project_agent_instructions(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
) -> Vec<String> {
    session
        .knowledge_base_ids
        .iter()
        .filter_map(|knowledge_base_id| {
            let knowledge_base = snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| &knowledge_base.id == knowledge_base_id)?;
            let instruction_path = PathBuf::from(&knowledge_base.path).join("ORANGE_AGENT.md");
            if !instruction_path.is_file() {
                return None;
            }
            match fs::read_to_string(&instruction_path) {
                Ok(content) => {
                    let bounded =
                        truncate_chars(content.trim(), MAX_PROJECT_AGENT_INSTRUCTION_CHARS);
                    if bounded.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "path: {}/ORANGE_AGENT.md\n来源知识库：{}（id={}）\n{}",
                            knowledge_base.path.trim_end_matches(['/', '\\']),
                            knowledge_base.name,
                            knowledge_base.id,
                            bounded
                        ))
                    }
                }
                Err(error) => {
                    log::warn!(
                        target: "agent_runtime",
                        "项目级 Agent 指令读取失败：knowledge_base_id={} error={}",
                        knowledge_base.id,
                        model_provider::redact_model_error_text(&error.to_string())
                    );
                    None
                }
            }
        })
        .collect()
}

pub(super) fn project_instruction_count(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
) -> usize {
    session
        .knowledge_base_ids
        .iter()
        .filter(|knowledge_base_id| {
            snapshot
                .knowledge_bases
                .iter()
                .find(|knowledge_base| &knowledge_base.id == *knowledge_base_id)
                .map(|knowledge_base| {
                    PathBuf::from(&knowledge_base.path)
                        .join("ORANGE_AGENT.md")
                        .is_file()
                })
                .unwrap_or(false)
        })
        .count()
}

fn render_knowledge_base_memory_prompt(
    memories: &[KnowledgeBaseMemory],
    knowledge_bases: &[crate::domain::KnowledgeBase],
) -> Option<String> {
    let mut lines = Vec::new();
    let mut entry_count = 0usize;
    for memory in memories.iter() {
        if !memory.enabled {
            continue;
        }
        let visible_entries: Vec<&AgentMemoryEntry> = memory
            .entries
            .iter()
            .filter(|entry| !entry.content.trim().is_empty())
            .take(MAX_RENDERED_KB_MEMORY_ENTRIES_PER_KB)
            .collect();
        if visible_entries.is_empty() {
            continue;
        }
        let kb_name = knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == memory.knowledge_base_id)
            .map(|knowledge_base| knowledge_base.name.as_str())
            .unwrap_or(&memory.knowledge_base_id);
        lines.push(format!("- 知识库：{}", kb_name));
        for entry in visible_entries {
            let redacted_content = crate::storage::redact_memory_secrets(entry.content.trim());
            let category = memory_category_label(&entry.category);
            lines.push(format!("  - [{category}] {redacted_content}"));
            entry_count += 1;
        }
    }
    if lines.is_empty() || entry_count == 0 {
        return None;
    }
    let body = lines.join("\n");
    Some(format!(
        "【跨会话记忆】\n以下是本知识库稳定的长期偏好与约定，优先级低于系统规则和项目指令，高于会话滚动记忆。仅作为持续生效的约定参考，不要逐条复述；如与用户本轮明确要求冲突，以用户本轮要求为准。\n{}",
        truncate_chars(&body, MAX_RENDERED_KB_MEMORY_CHARS)
    ))
}

fn memory_category_label(category: &str) -> String {
    match category {
        MEMORY_CATEGORY_NOTE_STRUCTURE => "笔记结构".to_owned(),
        MEMORY_CATEGORY_TAG_CONVENTION => "标签规范".to_owned(),
        MEMORY_CATEGORY_ORGANIZATION => "整理习惯".to_owned(),
        MEMORY_CATEGORY_CONVENTION => "知识库约定".to_owned(),
        MEMORY_CATEGORY_OTHER => "其他偏好".to_owned(),
        _ => "其他偏好".to_owned(),
    }
}

/** 解析本轮 @ 文件。客户端传入的 ID 不可信，必须重新按会话授权 scope 过滤。 */
pub(super) fn resolve_mentioned_files(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
    request: &AgentTurnRequest,
) -> Vec<MentionedFileMaterial> {
    let allowed_kb_ids: HashSet<&str> = session
        .knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect();
    let mut seen_ids = HashSet::new();
    let mut materials = Vec::new();
    let mut rejected_count = 0usize;
    let active_markdown = snapshot
        .notes
        .iter()
        .find(|note| note.id == request.active_note_id)
        .filter(|note| allowed_kb_ids.contains(note.knowledge_base_id.as_str()));

    for raw_id in &request.mentioned_file_ids {
        let file_id = raw_id.trim();
        if file_id.is_empty() || !seen_ids.insert(file_id.to_owned()) {
            continue;
        }
        if materials.len() >= MAX_MENTIONED_FILES_PER_TURN {
            rejected_count += 1;
            continue;
        }
        if let Some(note) = snapshot.notes.iter().find(|note| note.id == file_id) {
            if !allowed_kb_ids.contains(note.knowledge_base_id.as_str()) {
                rejected_count += 1;
                continue;
            }
            materials.push(MentionedFileMaterial {
                id: note.id.clone(),
                knowledge_base_id: note.knowledge_base_id.clone(),
                title: note.title.clone(),
                path: note.path.clone(),
                file_type: "markdown".to_owned(),
                content: Some(truncate_chars(&note.content, MAX_MENTIONED_TEXT_CHARS)),
                image_markdown_path: None,
            });
            continue;
        }
        let Some(document) = snapshot
            .documents
            .iter()
            .find(|document| document.id == file_id)
        else {
            rejected_count += 1;
            continue;
        };
        if !allowed_kb_ids.contains(document.knowledge_base_id.as_str())
            || !matches!(
                document.file_type.as_str(),
                "txt" | "docx" | "pdf" | "image"
            )
        {
            rejected_count += 1;
            continue;
        }
        let image_markdown_path = (document.file_type == "image")
            .then(|| {
                active_markdown.and_then(|markdown| {
                    (markdown.knowledge_base_id == document.knowledge_base_id)
                        .then(|| relative_markdown_path(&markdown.path, &document.path))
                        .flatten()
                })
            })
            .flatten();
        materials.push(MentionedFileMaterial {
            id: document.id.clone(),
            knowledge_base_id: document.knowledge_base_id.clone(),
            title: document.title.clone(),
            path: document.path.clone(),
            file_type: document.file_type.clone(),
            content: (document.file_type == "txt")
                .then(|| {
                    document
                        .content
                        .as_deref()
                        .map(|content| truncate_chars(content, MAX_MENTIONED_TEXT_CHARS))
                })
                .flatten(),
            image_markdown_path,
        });
    }

    if rejected_count > 0 {
        log::warn!(
            target: "agent_runtime",
            "显式 @ 文件已过滤：requested_count={} accepted_count={} rejected_count={}",
            request.mentioned_file_ids.len(),
            materials.len(),
            rejected_count
        );
    }
    materials
}

fn relative_markdown_path(markdown_path: &str, asset_path: &str) -> Option<String> {
    let markdown_directory = Path::new(markdown_path).parent()?;
    let source = normalized_path_components(markdown_directory)?;
    let target = normalized_path_components(Path::new(asset_path))?;
    let shared_length = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut result = vec!["..".to_owned(); source.len().saturating_sub(shared_length)];
    result.extend(target.into_iter().skip(shared_length));
    (!result.is_empty()).then(|| result.join("/"))
}

fn normalized_path_components(path: &Path) -> Option<Vec<String>> {
    let mut result = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => result.push(value.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

pub(super) fn render_mentioned_files_prompt(materials: &[MentionedFileMaterial]) -> Option<String> {
    if materials.is_empty() {
        return None;
    }
    let entries = materials
        .iter()
        .map(|material| {
            let metadata = format!(
                "- 文件：{}（id={}，类型={}，知识库={}，相对路径={}）",
                material.title,
                material.id,
                material.file_type,
                material.knowledge_base_id,
                material.path
            );
            if let Some(content) = &material.content {
                format!("{metadata}\n正文：\n{content}")
            } else if let Some(markdown_path) = &material.image_markdown_path {
                format!("{metadata}\n可插入当前 Markdown 的安全引用：![]({markdown_path})")
            } else {
                format!("{metadata}\n仅提供元数据；不要读取或上传二进制内容。")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!(
        "【本轮用户显式 @ 的文件】\n这些是本轮高优先级材料，请优先参考。它们不会缩小允许 scope：你仍可按需发现、读取或在待确认 diff 中修改 scope 内其他文件。当前编辑目标仍由界面当前文件决定。\n{entries}"
    ))
}

/** 估算模型消息字符数。 */
pub(super) fn estimate_model_messages_chars(messages: &[Value]) -> usize {
    messages
        .iter()
        .map(|message| message.to_string().chars().count())
        .sum()
}

/** 开发者预览截断长度；完整正文写入日志目录 JSON。 */
const PROMPT_DUMP_PREVIEW_CHARS: usize = 800;

/** 把即将发给模型的 messages 收成可落盘、可预览的转储。 */
pub(super) fn build_prompt_dump(
    session_id: &str,
    model_id: &str,
    model_context_length: Option<u64>,
    round: u32,
    kind: &str,
    messages: &[Value],
    recorded_at: &str,
) -> AgentPromptDump {
    let dump_messages = messages
        .iter()
        .enumerate()
        .map(|(index, message)| prompt_dump_message(index, message))
        .collect::<Vec<_>>();
    let total_chars = dump_messages.iter().map(|message| message.chars).sum();
    let outline = dump_messages
        .iter()
        .map(|message| format!("{}:{}", message.role, message.chars))
        .collect::<Vec<_>>()
        .join(",");

    AgentPromptDump {
        session_id: session_id.to_owned(),
        model_id: model_id.to_owned(),
        model_context_length,
        recorded_at: recorded_at.to_owned(),
        round,
        kind: kind.to_owned(),
        total_chars,
        file_path: String::new(),
        outline,
        messages: dump_messages,
    }
}

fn prompt_dump_message(index: usize, message: &Value) -> AgentPromptDumpMessage {
    let role = message_role(message).to_owned();
    let chars = message.to_string().chars().count();
    let content = serde_json::to_string_pretty(message).unwrap_or_else(|_| message.to_string());
    let preview_source = {
        let text = message_text_content(message);
        if text.is_empty() {
            content.clone()
        } else {
            text.to_owned()
        }
    };
    let (preview, truncated) = preview_chars(&preview_source, PROMPT_DUMP_PREVIEW_CHARS);
    AgentPromptDumpMessage {
        index,
        role,
        chars,
        preview,
        truncated,
        content: Some(content),
    }
}

/** 按字符截断预览，不附加预算提示。 */
pub(super) fn preview_chars(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_owned(), false);
    }
    (value.chars().take(max_chars).collect(), true)
}

/** 把字符串裁剪到指定字符预算。 */
pub(super) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let truncated = value.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n\n[内容已按上下文预算截断]")
}

/** 工具安全帽截断：尽量保持 JSON 完整，并带可执行下一步。 */
pub(super) fn truncate_tool_result_for_model(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let hint = "结果已截到 9000 字符。若是 read，用 offset=nextOffset 续读；若是 search/list，收窄 query 或提高 limit。";
    let truncated: String = value.chars().take(max_chars).collect();
    if serde_json::from_str::<Value>(&truncated).is_ok() {
        return format!("{truncated}\n\n{hint}");
    }
    let summary: String = value.chars().take(400).collect();
    json!({
        "truncated": true,
        "summary": summary,
        "hint": hint
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_transcript_rewrites_legacy_system_checkpoint_and_drops_other_system() {
        let transcript = vec![
            json!({ "role": "system", "content": "【会话工作记忆】\ncurrentGoal: 整理文章" }),
            json!({ "role": "system", "content": "旧项目指令不应回放" }),
            json!({ "role": "user", "content": "继续" }),
        ];
        let projected = project_transcript(&transcript, None);
        assert_eq!(projected[0]["role"], "user");
        assert!(projected[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("压缩检查点"));
        assert!(projected.iter().all(|message| message["role"] != "system"));
        assert!(projected.iter().any(|message| message["content"]
            .as_str()
            .unwrap_or_default()
            .contains("继续")));
    }

    #[test]
    fn filter_incomplete_tool_pairs_drops_unpaired_assistant() {
        let messages = vec![
            json!({ "role": "user", "content": "检索" }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "id": "missing", "type": "function", "function": { "name": "search", "arguments": "{}" } }]
            }),
            json!({ "role": "assistant", "content": "已完成" }),
        ];
        let filtered = filter_incomplete_tool_pairs(&messages);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[1]["content"], "已完成");
        assert!(filtered
            .iter()
            .all(|message| message.get("tool_calls").is_none()));
    }

    #[test]
    fn content_null_becomes_empty_string() {
        let projected =
            project_transcript(&[json!({ "role": "assistant", "content": null })], None);
        assert_eq!(projected[0]["content"], "");
    }

    #[test]
    fn truncate_tool_result_keeps_json_or_emits_truncated_object() {
        let valid = format!(r#"{{"ok":true,"body":"{}"}}"#, "x".repeat(50));
        assert!(
            serde_json::from_str::<Value>(&truncate_tool_result_for_model(&valid, 10_000)).is_ok()
        );
        let broken = format!(r#"{{"ok":true,"body":"{}"#, "y".repeat(20_000));
        let truncated = truncate_tool_result_for_model(&broken, 9000);
        let parsed: Value = serde_json::from_str(&truncated).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert!(parsed["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("offset=nextOffset"));
    }

    #[test]
    fn group_transcript_units_keeps_tool_pairs_together() {
        let messages = vec![
            json!({ "role": "user", "content": "旧" }),
            json!({
                "role": "assistant",
                "content": "检索",
                "tool_calls": [{ "id": "call-1", "type": "function", "function": { "name": "search", "arguments": "{}" } }]
            }),
            json!({ "role": "tool", "tool_call_id": "call-1", "content": "result" }),
            json!({ "role": "user", "content": "新" }),
        ];
        let units = group_transcript_units(&messages);
        assert_eq!(units.len(), 3);
        assert_eq!(units[1].len(), 2);
        assert_eq!(units[1][1]["tool_call_id"], "call-1");
    }

    #[test]
    fn prompt_dump_outline_lists_roles_and_keeps_full_content_for_file() {
        let messages = vec![
            json!({ "role": "system", "content": "角色说明" }),
            json!({ "role": "user", "content": "请整理这篇笔记" }),
        ];
        let dump = build_prompt_dump(
            "session-a",
            "gpt-4o-mini",
            Some(128_000),
            1,
            "turn",
            &messages,
            "2026-08-28 10:00:00",
        );

        assert_eq!(dump.session_id, "session-a");
        assert_eq!(dump.model_context_length, Some(128_000));
        assert!(dump.outline.contains("system:"));
        assert!(dump.outline.contains("user:"));
        assert_eq!(dump.messages.len(), 2);
        assert!(dump.messages[0]
            .content
            .as_ref()
            .is_some_and(|content| content.contains("角色说明")));
        assert!(!dump.messages[1].truncated);
    }

    #[test]
    fn prompt_dump_preview_truncates_long_content() {
        let long_content = "很长的系统提示".repeat(200);
        let messages = vec![json!({ "role": "system", "content": long_content })];
        let dump = build_prompt_dump(
            "session-a",
            "gpt-4o-mini",
            None,
            2,
            "turn",
            &messages,
            "2026-08-28 10:00:00",
        );

        assert!(dump.messages[0].truncated);
        assert_eq!(
            dump.messages[0].preview.chars().count(),
            PROMPT_DUMP_PREVIEW_CHARS
        );
        assert!(dump.messages[0]
            .content
            .as_ref()
            .is_some_and(|content| content.chars().count() > PROMPT_DUMP_PREVIEW_CHARS));
    }
}
