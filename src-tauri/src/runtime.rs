use crate::agent;
use crate::agent_tools::{AgentToolContext, ToolRegistry};
use crate::domain::{
    AgentContextSummary, AgentContextTouchedNote, AgentMemoryEntry, AgentMessage, AgentSession,
    AgentSkill, AgentToolCall, AgentTurnRequest, AgentTurnResult, Citation, KnowledgeBaseMemory,
    LlmProviderConfig, ProposedChange, RequestAuditLog, UserSettings, WorkspaceSnapshot,
    MEMORY_CATEGORY_CONVENTION, MEMORY_CATEGORY_NOTE_STRUCTURE, MEMORY_CATEGORY_ORGANIZATION,
    MEMORY_CATEGORY_OTHER, MEMORY_CATEGORY_TAG_CONVENTION,
};
use crate::model_provider;
use crate::skills;
use crate::storage::{create_id, format_local_datetime};
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/** 模型最多读取的历史消息数量，避免长会话在 M3 首版阶段无限膨胀上下文。 */
const MAX_MODEL_HISTORY_MESSAGES: usize = 8;

/** 单条历史消息进入模型前的最大字符数。 */
const MAX_HISTORY_MESSAGE_CHARS: usize = 1200;

/** 工作记忆渲染进 prompt 的最大字符数，避免 summary 自身吞掉上下文预算。 */
const MAX_RENDERED_CONTEXT_SUMMARY_CHARS: usize = 6000;

/** 单条工作记忆字段的最大字符数，避免模型总结把正文塞进结构化字段。 */
const MAX_CONTEXT_SUMMARY_ITEM_CHARS: usize = 360;

/** 工作记忆每个数组字段最多保留的条目数。 */
const MAX_CONTEXT_SUMMARY_ITEMS: usize = 12;

/** 项目级 Agent 指令最大读取字符数；只读取知识库根目录 ORANGE_AGENT.md。 */
const MAX_PROJECT_AGENT_INSTRUCTION_CHARS: usize = 16 * 1024;

/** 跨会话记忆渲染进 prompt 的最大字符数，低于会话工作记忆预算以优先保留滚动上下文。 */
const MAX_RENDERED_KB_MEMORY_CHARS: usize = 4000;

/** 单个知识库记忆渲染进 prompt 时最多保留的条目数，避免一个 KB 挤占预算。 */
const MAX_RENDERED_KB_MEMORY_ENTRIES_PER_KB: usize = 8;

/** 手动和自动整理上下文时，最多把多少条未总结消息交给总结器。 */
const MAX_RECENT_MESSAGES_FOR_SUMMARY: usize = 12;

/** 超过该消息数后自动触发模型整理，避免长会话仅依赖短期历史。 */
const AUTO_COMPACT_MESSAGE_COUNT_THRESHOLD: usize = 16;

/** 最近未进入 summary 的消息超过该数量时自动整理。 */
const AUTO_COMPACT_UNSUMMARIZED_MESSAGE_THRESHOLD: usize = 8;

/** 估算 prompt 字符数超过该阈值时自动整理，避免请求上下文持续膨胀。 */
const AUTO_COMPACT_PROMPT_CHAR_THRESHOLD: usize = 18_000;

/** 工具结果回填给模型时的最大 JSON 字符数。 */
const MAX_TOOL_RESULT_CHARS: usize = 9000;

/** 请求审计最多记录的发送片段摘要数量。 */
const MAX_AUDIT_FRAGMENTS: usize = 8;

/** 后端再次限制每轮显式 Skill 数量，避免绕过 UI 传入过多 instructions。 */
const MAX_EXPLICIT_SKILLS_PER_TURN: usize = 3;

/** 单轮最多接受的显式 @ 文件数量，避免客户端绕过输入框造成上下文膨胀。 */
const MAX_MENTIONED_FILES_PER_TURN: usize = 8;

/** 单个显式文本材料的正文上限；正文不会写入日志。 */
const MAX_MENTIONED_TEXT_CHARS: usize = 12_000;

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

/** 已通过 scope 校验的本轮显式材料，内容仅在构造模型消息时使用。 */
struct MentionedFileMaterial {
    id: String,
    knowledge_base_id: String,
    title: String,
    path: String,
    file_type: String,
    content: Option<String>,
    image_markdown_path: Option<String>,
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
struct ContextSummaryAutoDecision {
    should_compact: bool,
    reasons: Vec<String>,
    estimated_prompt_chars: usize,
    unsummarized_message_count: usize,
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
            render_context_summary_prompt(session.context_summary.as_ref())
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

/**
 * 解析本轮 @ 文件。客户端传入的 ID 不可信，必须重新按会话授权 scope、文件类型和数量限制过滤。
 * Markdown/TXT 使用已索引的文本正文；二进制和 Office 文档只向模型提供元数据。
 */
fn resolve_mentioned_files(
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

    // 图片链接只在当前编辑目标为同一知识库 Markdown 时生成，避免跨根目录产生失效引用。
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
            request.mentioned_file_ids.len(), materials.len(), rejected_count
        );
    }
    log::debug!(
        target: "agent_runtime",
        "显式 @ 文件解析完成：requested_count={} accepted_count={} text_count={} metadata_count={}",
        request.mentioned_file_ids.len(),
        materials.len(),
        materials.iter().filter(|material| material.content.is_some()).count(),
        materials.iter().filter(|material| material.content.is_none()).count()
    );
    materials
}

/** 为同一知识库中的图片计算相对当前 Markdown 的安全引用路径，不访问本机绝对路径。 */
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

/** 将知识库扫描得到的相对路径规范化为普通组件，拒绝上级目录和绝对路径。 */
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

/** 将已校验的显式材料渲染给模型，并强调它们不缩小既有工具 scope。 */
fn render_mentioned_files_prompt(materials: &[MentionedFileMaterial]) -> Option<String> {
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

/** 手动整理指定会话上下文；真实模型不可用时降级为本地确定性整理。 */
pub async fn compact_agent_context_summary(
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
        selected_model_id.clone(),
        api_key,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => model_error_turn(
            snapshot,
            request,
            Some(&provider),
            Some(&selected_model_id),
            &available_skills,
            &explicit_skills,
            &format!("模型请求失败：{error}"),
        ),
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
    let mut model_messages = build_model_messages(
        &snapshot,
        session_index,
        &request,
        &available_skills,
        &explicit_skills,
        &current_user_message_id,
        &kb_memories,
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

    tool_calls.push(model_request_tool_call(
        &provider,
        &selected_model_id,
        &endpoint,
        "completed",
    ));

    log::info!(
        target: "agent_runtime",
        "模型 Agent 自主工具选择开始：session={} action={} provider_id={} provider_name={} model={} enabled_skill_count={} explicit_skill_count={} explicit_instruction_chars={} scope_count={} prompt_chars={}",
        snapshot.sessions[session_index].id,
        request.action,
        provider.id,
        provider.name,
        selected_model_id,
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
            &selected_model_id,
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
            update_agent_context_summary_after_turn(
                &client,
                &provider,
                &selected_model_id,
                &api_key,
                &mut snapshot,
                session_index,
                estimate_model_messages_chars(&model_messages),
                last_failed_tool_summary.as_deref(),
            )
            .await;
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
        &selected_model_id,
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
    update_agent_context_summary_after_turn(
        &client,
        &provider,
        &selected_model_id,
        &api_key,
        &mut snapshot,
        session_index,
        estimate_model_messages_chars(&model_messages),
        last_failed_tool_summary.as_deref(),
    )
    .await;
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
    knowledge_base_memories: &[KnowledgeBaseMemory],
) -> Vec<Value> {
    let session = &snapshot.sessions[session_index];
    // Agent 的工具选择策略只作为模型指令，不再由宿主预判用户意图。
    let autonomous_tool_policy = "你需要根据用户输入和上下文自主判断是否调用工具：需要 Markdown 引用时使用 search_notes；需要当前 scope 内 Markdown/TXT 正文或写入建议时使用 read_file、get_current_file、propose_file_change；需要 DOCX/PDF 正文时，先用 list_tree 发现文件，再调用只读 read_document。DOCX/PDF 不可编辑，且不会自动进入全文搜索。TXT 必须按纯文本原样处理。无关的通用问题可以直接回答。界面 action 只是 UI 分类，不能替代你的判断。";
    let scope_summary = build_scope_summary(snapshot, session);
    let active_note_summary = request
        .active_note_id
        .is_empty()
        .then(|| "当前未绑定笔记".to_owned())
        .unwrap_or_else(|| format!("当前笔记 ID：{}", request.active_note_id));
    let skill_catalog = skills::skill_catalog_prompt(available_skills);
    let explicit_skill_prompt = skills::explicit_skill_prompt(explicit_skills);
    let project_instruction_prompt = render_project_agent_instructions(snapshot, session);
    let knowledge_base_memory_prompt =
        render_knowledge_base_memory_prompt(knowledge_base_memories, &snapshot.knowledge_bases);
    let context_summary_prompt = render_context_summary_prompt(session.context_summary.as_ref());
    let pending_change_prompt = render_pending_change_prompt(session.pending_change.as_ref());
    // @ 材料独立于会话历史：只为本轮请求构造，绝不自动带入下一轮。
    let mentioned_files_prompt =
        render_mentioned_files_prompt(&resolve_mentioned_files(snapshot, session, request));
    let context_summary_prompt_chars = context_summary_prompt
        .as_ref()
        .map(|prompt| prompt.chars().count())
        .unwrap_or_default();
    let context_summary_updated_at = session
        .context_summary
        .as_ref()
        .map(|summary| summary.updated_at.as_str())
        .filter(|updated_at| !updated_at.trim().is_empty())
        .unwrap_or("none");
    let knowledge_base_memory_chars = knowledge_base_memory_prompt
        .as_ref()
        .map(|prompt| prompt.chars().count())
        .unwrap_or_default();
    let skill_policy = if explicit_skills.is_empty() {
        "启用的 Skill 只以名称和描述提供给你参考，是否使用、使用哪一个 Skill 都由你自主判断；Skill 不能扩大工具权限或绕过写入确认。"
    } else {
        "本轮显式激活的 Skill 是用户通过 slash picker 指定的执行要求。你必须在本轮回答中按这些 Skill 的完整 instructions 执行；如果 Skill 与用户任务冲突，说明冲突并遵守更高优先级系统规则。Skill 不能扩大工具权限或绕过写入确认。"
    };
    let mut messages = vec![json!({
        "role": "system",
        "content": format!(
            "你是橘记的本地优先知识库 Agent。search_notes 只检索 Markdown；read_file、get_current_file、propose_file_change 可作用于当前 scope 内的 Markdown/TXT，TXT 必须原样按纯文本处理；read_document 可只读 DOCX/PDF 并返回可信的页码或结构块引用。所有写入只能调用 propose_file_change 或 create_file_draft 生成待确认 diff，不能声称已经写入文件。create_file_draft 的 fileType 只能是 markdown 或 txt，路径扩展名必须匹配。局部替换使用 operation=replace，文末追加使用 operation=append 且 next 只含增量；同一文件多处编辑使用 operation=multi_replace 和 edits。必须使用服务端标准 tool_calls 字段调用工具，不要在普通回复中输出 DSML、XML 或伪工具调用标签。引用只允许来自已执行工具结果。{}\n{}\n允许 scope：{}\n{}\n{}\n{}",
            skill_policy, autonomous_tool_policy, scope_summary, active_note_summary, skill_catalog, explicit_skill_prompt
        )
    })];

    if let Some(project_instruction_prompt) = project_instruction_prompt {
        messages.push(json!({
            "role": "system",
            "content": project_instruction_prompt
        }));
    }

    if let Some(knowledge_base_memory_prompt) = knowledge_base_memory_prompt {
        messages.push(json!({
            "role": "system",
            "content": knowledge_base_memory_prompt
        }));
    }

    if let Some(context_summary_prompt) = context_summary_prompt {
        messages.push(json!({
            "role": "system",
            "content": context_summary_prompt
        }));
    }

    if let Some(pending_change_prompt) = pending_change_prompt {
        messages.push(json!({
            "role": "system",
            "content": pending_change_prompt
        }));
    }

    if let Some(mentioned_files_prompt) = mentioned_files_prompt {
        messages.push(json!({
            "role": "system",
            "content": mentioned_files_prompt
        }));
    }

    log::debug!(
        target: "agent_runtime",
        "上下文注入完成：session={} summary_injected={} summary_chars={} summary_updated_at={} has_pending_change={} project_instruction_count={} kb_memory_injected={} kb_memory_chars={} kb_memory_entry_count={} kb_memory_kb_count={}",
        session.id,
        context_summary_prompt_chars > 0,
        context_summary_prompt_chars,
        context_summary_updated_at,
        session.pending_change.as_ref().is_some_and(|change| change.status == "pending"),
        project_instruction_count(snapshot, session),
        knowledge_base_memory_chars > 0,
        knowledge_base_memory_chars,
        knowledge_base_memories.iter().map(|memory| memory.entries.len()).sum::<usize>(),
        knowledge_base_memories.len()
    );

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

/** 渲染知识库根目录 ORANGE_AGENT.md 指令；读取失败只写脱敏日志，不阻塞 Agent 回合。 */
fn render_project_agent_instructions(
    snapshot: &WorkspaceSnapshot,
    session: &AgentSession,
) -> Option<String> {
    let instructions = load_project_agent_instructions(snapshot, session);

    if instructions.is_empty() {
        return None;
    }

    Some(format!(
        "【项目级 Agent 指令】\n以下内容来自当前会话 scope 内知识库根目录的 ORANGE_AGENT.md，优先级低于橘记系统规则，高于普通会话记忆。\n{}",
        instructions.join("\n\n")
    ))
}

/** 读取当前会话 scope 内的项目级 Agent 指令，只按知识库根目录固定文件名加载。 */
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

            // 项目级指令可由用户编辑，读取时只按字符预算截断，不做正文日志输出。
            match fs::read_to_string(&instruction_path) {
                Ok(content) => {
                    let bounded =
                        truncate_chars(content.trim(), MAX_PROJECT_AGENT_INSTRUCTION_CHARS);

                    if bounded.is_empty() {
                        None
                    } else {
                        Some(format!(
                            "来源知识库：{}（id={}）\n{}",
                            knowledge_base.name, knowledge_base.id, bounded
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

/** 统计本轮注入了多少份项目级指令，用于 debug 日志，不重复记录正文。 */
fn project_instruction_count(snapshot: &WorkspaceSnapshot, session: &AgentSession) -> usize {
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

/** 渲染会话工作记忆 prompt；summary 为空时不注入，避免制造无意义上下文。 */
fn render_context_summary_prompt(summary: Option<&AgentContextSummary>) -> Option<String> {
    let summary = summary?;
    let body = render_context_summary_body(summary);

    if body.trim().is_empty() {
        return None;
    }

    Some(format!(
        "【会话工作记忆】\n以下是本会话较早上下文的压缩摘要，优先级低于系统规则，高于最近历史之外的普通聊天记忆。\n{}",
        truncate_chars(&body, MAX_RENDERED_CONTEXT_SUMMARY_CHARS)
    ))
}

/** 把结构化工作记忆转成模型可读文本，保持字段稳定便于模型增量合并。 */
fn render_context_summary_body(summary: &AgentContextSummary) -> String {
    let mut lines = vec![
        format!("version: {}", summary.version),
        format!("updatedAt: {}", summary.updated_at),
    ];

    if let Some(goal) = summary
        .current_goal
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        lines.push(format!("currentGoal: {}", goal));
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

    if let Some(message_id) = summary
        .last_summarized_message_id
        .as_deref()
        .filter(|message_id| !message_id.trim().is_empty())
    {
        lines.push(format!("lastSummarizedMessageId: {message_id}"));
    }

    if let Some(message_id) = summary
        .last_compacted_message_id
        .as_deref()
        .filter(|message_id| !message_id.trim().is_empty())
    {
        lines.push(format!("lastCompactedMessageId: {message_id}"));
    }

    lines.join("\n")
}

/** 只计算工作记忆渲染长度，供日志和审计记录使用，不暴露 summary 正文。 */
fn context_summary_rendered_chars(summary: Option<&AgentContextSummary>) -> usize {
    summary
        .map(render_context_summary_body)
        .map(|body| body.chars().count())
        .unwrap_or_default()
}

/** 追加工作记忆数组字段，空数组不输出以节省 prompt 预算。 */
fn push_summary_list(lines: &mut Vec<String>, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }

    lines.push(format!("{label}:"));
    for item in items.iter().take(MAX_CONTEXT_SUMMARY_ITEMS) {
        lines.push(format!("- {item}"));
    }
}

/** 渲染待确认 diff 摘要，只暴露状态和统计，不把 original/next 正文放进 prompt。 */
fn render_pending_change_prompt(change: Option<&ProposedChange>) -> Option<String> {
    let change = change?;
    if change.status != "pending" {
        return None;
    }

    let summary = summarize_pending_change(change)?;

    Some(format!(
        "【当前待确认变更】\n以下是当前会话的 diff 状态摘要。不要把它当成已写入文件；只有用户确认后才会落盘。\n{summary}"
    ))
}

/** 渲染已启用的跨会话记忆；全部为空或未启用时不注入，避免制造无意义上下文。 */
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
            // 生成前再次脱敏，防止旧数据或手动改库绕过保存入口后进入模型上下文。
            let redacted_content = crate::storage::redact_memory_secrets(entry.content.trim());
            let category = memory_category_label(&entry.category);
            lines.push(format!("  - [{}] {}", category, redacted_content));
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

/** 把记忆条目内部 category 值映射成模型可读的中文标签，未识别值降级为其他偏好。 */
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

/** 生成 pending change 的脱敏摘要，供 prompt、summary 和日志复用。 */
fn summarize_pending_change(change: &ProposedChange) -> Option<String> {
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

/** 当前仍等待确认的 diff 摘要；accepted/rejected 不再作为待确认状态注入模型。 */
fn current_pending_change_summary(session: &AgentSession) -> Option<String> {
    session
        .pending_change
        .as_ref()
        .filter(|change| change.status == "pending")
        .and_then(summarize_pending_change)
}

/** 发送一次 chat completions 请求并记录 providerId/model/status/耗时/endpointHost；错误统一脱敏。 */
async fn send_chat_completion_logged(
    client: &Client,
    provider: &LlmProviderConfig,
    model_id: &str,
    endpoint: &str,
    api_key: &str,
    messages: &[Value],
    include_tools: bool,
) -> Result<Value, String> {
    let started_at = Instant::now();
    let result =
        send_chat_completion(client, endpoint, api_key, model_id, messages, include_tools).await;

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
    let result = send_chat_completion_logged(
        client,
        provider,
        selected_model_id,
        &endpoint,
        api_key,
        &messages,
        false,
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
            let rendered_chars = render_context_summary_body(&summary).chars().count();
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
) {
    let decision =
        context_summary_auto_decision(&snapshot.sessions[session_index], estimated_prompt_chars);

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
}

/** 计算 自动整理触发条件，返回原因列表供日志观测。 */
fn context_summary_auto_decision(
    session: &AgentSession,
    estimated_prompt_chars: usize,
) -> ContextSummaryAutoDecision {
    let mut reasons = Vec::new();
    let unsummarized_message_count = compact_unsummarized_message_count(session);
    let has_compacted = session
        .context_summary
        .as_ref()
        .and_then(|summary| summary.last_compacted_message_id.as_deref())
        .is_some();

    if session.context_summary.is_none() && !session.messages.is_empty() {
        reasons.push("firstSummary".to_owned());
    }

    if session.messages.len() > AUTO_COMPACT_MESSAGE_COUNT_THRESHOLD && !has_compacted {
        reasons.push("messageCountOverThreshold".to_owned());
    }

    if unsummarized_message_count > AUTO_COMPACT_UNSUMMARIZED_MESSAGE_THRESHOLD {
        reasons.push("unsummarizedMessagesOverThreshold".to_owned());
    }

    if estimated_prompt_chars > AUTO_COMPACT_PROMPT_CHAR_THRESHOLD && unsummarized_message_count > 0
    {
        reasons.push("promptCharsOverThreshold".to_owned());
    }

    if pending_change_summary_changed(session) {
        reasons.push("pendingChangeChanged".to_owned());
    }

    ContextSummaryAutoDecision {
        should_compact: !reasons.is_empty(),
        reasons,
        estimated_prompt_chars,
        unsummarized_message_count,
    }
}

/** 估算模型消息字符数，用于触发 prompt 过大时的自动 compact。 */
fn estimate_model_messages_chars(messages: &[Value]) -> usize {
    messages
        .iter()
        .map(|message| message.to_string().chars().count())
        .sum()
}

/** 统计距离上一次模型 compact 后新增的消息数；确定性同步不会重置该计数。 */
fn compact_unsummarized_message_count(session: &AgentSession) -> usize {
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

/** 当前待确认 diff 与 summary 内记录不一致时触发整理，避免模型忘记待确认状态。 */
fn pending_change_summary_changed(session: &AgentSession) -> bool {
    let recorded = session
        .context_summary
        .as_ref()
        .and_then(|summary| summary.pending_change_summary.as_deref());
    let current = current_pending_change_summary(session);

    recorded != current.as_deref()
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

    serde_json::from_str::<AgentContextSummary>(&json_text)
        .map_err(|error| format!("summary JSON 解析失败：{error}"))
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

    let rendered_chars = render_context_summary_body(&summary).chars().count();
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
            mentioned_file_ids: Vec::new(),
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
            mentioned_file_ids: Vec::new(),
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
        content_summary: audit_trail.content_summary(content_summary, prompt, session),
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
                im_identity: None,
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: vec!["note-a".to_owned()],
                messages: Vec::new(),
                pending_change: None,
                context_summary: None,
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
                deleted_at: None,
                model_provider_id: None,
                model_id: None,
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
            model_id: None,
            explicit_skill_ids: Vec::new(),
            mentioned_file_ids: Vec::new(),
        }
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
        assert!(system_content.contains("可用 Skills"));
        assert!(system_content.contains("仅名称和描述"));
        assert!(system_content.contains("知识库研究"));
        assert!(system_content.contains("是否使用、使用哪一个 Skill 都由你自主判断"));
        assert!(!system_content.contains("执行要求"));
        assert!(!system_content.contains("当用户要求查找"));
    }

    /** 会话工作记忆必须在 system 指令之后、短期历史之前注入，确保长会话目标不被最近 8 条限制丢掉。 */
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
        snapshot.sessions[0].messages.push(AgentMessage {
            id: "user-current".to_owned(),
            role: "user".to_owned(),
            content: "继续处理".to_owned(),
            action: Some("ask".to_owned()),
            citations: None,
            tool_calls: None,
            mentioned_file_ids: Vec::new(),
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
        let memory_content = messages[1]["content"].as_str().unwrap_or_default();

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "system");
        assert!(memory_content.contains("【会话工作记忆】"));
        assert!(memory_content.contains("按产品分析框架整理这篇文章"));
        assert_eq!(messages[2]["role"], "user");
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

        let memory_content = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .find(|content| content.contains("【跨会话记忆】"))
            .unwrap_or_default();
        assert!(!memory_content.is_empty());
        assert!(
            memory_content.chars().count() <= MAX_RENDERED_KB_MEMORY_CHARS + 200,
            "跨会话记忆渲染应被截断到预算上限附近，实际 {} 字符",
            memory_content.chars().count()
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

        snapshot.sessions[0].messages = (0..18)
            .map(|index| AgentMessage {
                id: format!("message-{index}"),
                role: if index % 2 == 0 { "user" } else { "assistant" }.to_owned(),
                content: format!("消息 {index}"),
                action: Some("ask".to_owned()),
                citations: None,
                tool_calls: None,
                mentioned_file_ids: Vec::new(),
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
            last_summarized_message_id: Some("message-17".to_owned()),
            last_compacted_message_id: Some("message-0".to_owned()),
        });
        snapshot.sessions[0].pending_change = Some(runtime_test_pending_change("pending"));

        let decision = context_summary_auto_decision(
            &snapshot.sessions[0],
            AUTO_COMPACT_PROMPT_CHAR_THRESHOLD + 1,
        );

        assert!(decision.should_compact);
        assert!(decision
            .reasons
            .contains(&"unsummarizedMessagesOverThreshold".to_owned()));
        assert!(decision
            .reasons
            .contains(&"promptCharsOverThreshold".to_owned()));
        assert!(decision
            .reasons
            .contains(&"pendingChangeChanged".to_owned()));
        assert_eq!(decision.unsummarized_message_count, 17);
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
            &[],
        );
        let system_content = messages[0]["content"].as_str().unwrap_or_default();

        assert!(system_content.contains("本轮显式激活的 Skills"));
        assert!(system_content.contains("执行要求"));
        assert!(system_content.contains(&explicit_skill.instructions));
        assert!(system_content.contains("可用 Skills"));
        assert!(system_content.contains("不能扩大工具权限"));
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
        });
        snapshot.sessions[0].pending_change = Some(runtime_test_pending_change("pending"));

        update_agent_context_summary_deterministic(
            &mut snapshot,
            0,
            Some("read_note 工具失败：目标笔记不在 scope 内"),
            false,
        );

        let summary = snapshot.sessions[0].context_summary.as_ref().unwrap();
        let rendered = render_context_summary_body(summary);

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
