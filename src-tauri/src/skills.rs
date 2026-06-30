use crate::domain::{
    AgentSkill, AgentTurnRequest, InstallAgentSkillResult, SkillSettings, UserSettings,
};
use crate::storage::{create_id, format_local_datetime, lock_database_writer};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, Manager};
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::ZipArchive;

/** 内置 skill 来源标记，决定设置页只能禁用不能删除。 */
pub const BUILT_IN_SKILL_SOURCE: &str = "built-in";

/** 旧版用户自建 skill 来源标记；新建 skill 会迁移为文件式 SKILL.md。 */
pub const USER_SKILL_SOURCE: &str = "user";

/** 文件式 skill 来源标记，来自 Cici Note 用户目录中的 SKILL.md。 */
pub const FILE_SKILL_SOURCE: &str = "file";

/** Cici Note 自有用户目录名，避免污染或误读用户真实 Codex 配置。 */
const CICI_HOME_DIRECTORY_NAME: &str = ".cici-note";

/** 文件式 skill 的主说明文件名，沿用 Codex 的目录化体验。 */
const SKILL_MARKDOWN_FILE_NAME: &str = "SKILL.md";

/** 预留记忆目录名，首版只创建但不读取、不注入 Runtime。 */
const MEMORY_DIRECTORY_NAME: &str = "memory";

/** Skill 摘要目录注入模型的最大字符数，避免用户安装大量 skill 后挤占上下文。 */
const MAX_SKILL_CATALOG_PROMPT_CHARS: usize = 8_000;

/** 第三方 skill 安装时允许复制的最大普通文件数量。 */
const MAX_SKILL_INSTALL_FILE_COUNT: usize = 512;

/** 第三方 skill 安装时允许复制的单文件最大字节数。 */
const MAX_SKILL_INSTALL_SINGLE_FILE_BYTES: u64 = 5 * 1024 * 1024;

/** 第三方 skill 安装时允许复制的总字节数。 */
const MAX_SKILL_INSTALL_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

/** 远程下载的单个 SKILL.md 最大字节数。 */
pub const MAX_REMOTE_SKILL_MARKDOWN_BYTES: usize = 1024 * 1024;

/** 远程下载的压缩包最大字节数；解压后还会再次做总量限制。 */
pub const MAX_REMOTE_SKILL_ARCHIVE_BYTES: usize = 25 * 1024 * 1024;

/** 第三方 skill 安装时保存在 agents 目录中的 Cici Note 元数据文件。 */
const CICI_INSTALL_METADATA_FILE_NAME: &str = "cici-note.yaml";

/** 第三方 skill 安装冲突时直接失败，不覆盖用户现有目录。 */
const INSTALL_CONFLICT_FAIL: &str = "fail";

/** 第三方 skill 安装冲突时替换同名目录。 */
const INSTALL_CONFLICT_REPLACE: &str = "replace";

/** 读取全部内置 skill，首版固定为指令型工作流，不携带脚本或外部命令。 */
pub fn built_in_skills() -> Vec<AgentSkill> {
    vec![
        built_in_skill(
            "skill-note-research",
            "note-research",
            "知识库研究",
            "基于已选知识库检索、阅读笔记，并给出带引用的回答。",
            "当用户要求查找、总结、对比或引用本地笔记时，先调用 search_notes、read_note 或 list_tree 获取依据。回答中只引用工具返回的材料；如果工具没有结果，明确说明未找到依据，不要编造来源。",
            &["研究", "检索", "引用"],
            &["查找", "搜索", "检索", "引用", "来源", "总结", "知识库", "笔记", "资料"],
        ),
        built_in_skill(
            "skill-note-rewrite",
            "note-rewrite",
            "笔记改写",
            "改写当前笔记内容，并通过待确认 diff 交给用户决定是否写入。",
            "当用户要求润色、改写、压缩或扩写当前笔记时，先读取当前笔记或目标笔记。只能调用 propose_note_change 生成待确认 diff；不能声称已经修改文件，也不能绕过 original 唯一命中校验。",
            &["写作", "改写", "diff"],
            &["改写", "润色", "重写", "优化", "扩写", "压缩", "rewrite"],
        ),
        built_in_skill(
            "skill-draft-from-context",
            "draft-from-context",
            "上下文草稿",
            "基于已选 scope 创建新的 Markdown 草稿，写入前仍需用户确认。",
            "当用户要求生成新笔记、清单、总结稿或草稿时，可以先检索或读取相关笔记，再调用 create_note_draft。目标路径必须在当前会话允许的知识库内，正文应是完整 Markdown。",
            &["草稿", "生成", "Markdown"],
            &["创建", "新建", "草稿", "生成", "清单", "draft", "markdown"],
        ),
        built_in_skill(
            "skill-organize-knowledge",
            "organize-knowledge",
            "知识整理",
            "给出标签、标题、目录和关联笔记建议，不直接移动或改写文件。",
            "当用户要求整理知识库、补标签、规划目录或建立关联时，优先调用 list_tree、search_notes 或 read_note 获取结构与内容，再调用 suggest_organization 输出建议。该 skill 不执行文件移动或直接写入。",
            &["整理", "标签", "目录"],
            &["整理", "归档", "标签", "目录", "分类", "关联", "组织", "organize"],
        ),
    ]
}

/** 获取用户文件式 skills 根目录，并创建预留 memory 目录。 */
pub fn user_skills_root(app: &AppHandle) -> Result<PathBuf, String> {
    let cici_home = user_cici_home(app)?;
    let skills_root = cici_home.join("skills");
    let memory_root = cici_home.join(MEMORY_DIRECTORY_NAME);

    fs::create_dir_all(&skills_root).map_err(|error| {
        format!(
            "无法创建用户 Skills 目录 {}：{error}",
            skills_root.display()
        )
    })?;
    fs::create_dir_all(&memory_root).map_err(|error| {
        format!(
            "无法创建用户 memory 预留目录 {}：{error}",
            memory_root.display()
        )
    })?;

    Ok(skills_root)
}

/** 从 SQLite 和用户目录读取 skill，并按内置、文件、用户自建顺序合并。 */
pub fn load_agent_skills(
    app: &AppHandle,
    connection: &Connection,
) -> Result<Vec<AgentSkill>, String> {
    let skills_root = user_skills_root(app)?;

    load_agent_skills_from_roots(connection, &[skills_root])
}

/** 从指定目录读取 skill，测试可传入临时根目录模拟 ~/.cici-note/skills。 */
pub fn load_agent_skills_from_roots(
    connection: &Connection,
    file_skill_roots: &[PathBuf],
) -> Result<Vec<AgentSkill>, String> {
    let mut persisted_skills = read_persisted_skills(connection)?;
    let mut skills = built_in_skills()
        .into_iter()
        .map(|mut skill| {
            if let Some(saved_skill) = persisted_skills.remove(&skill.id) {
                // 内置 skill 的说明始终以代码版本为准，只继承用户启停和模型参考偏好。
                skill.enabled = saved_skill.enabled;
                skill.allow_auto_invoke = saved_skill.allow_auto_invoke;
                skill.updated_at = saved_skill.updated_at;
            }

            skill
        })
        .collect::<Vec<_>>();

    if let Some(primary_root) = file_skill_roots.first() {
        migrate_legacy_user_skills(connection, primary_root, &mut persisted_skills);
    }

    let mut file_skills = scan_file_skills(file_skill_roots, &mut persisted_skills);

    file_skills.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    skills.extend(file_skills);

    let mut user_skills = persisted_skills
        .into_values()
        .filter(|skill| skill.source == USER_SKILL_SOURCE)
        .map(normalize_user_skill)
        .collect::<Result<Vec<_>, String>>()?;

    user_skills.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    skills.extend(user_skills);

    Ok(skills)
}

/** 保存用户自建 skill 到 Cici Note skills 目录，SQLite 只记录启停覆盖。 */
pub fn save_user_skill(
    app: &AppHandle,
    connection: &Connection,
    skill: AgentSkill,
) -> Result<AgentSkill, String> {
    let skills_root = user_skills_root(app)?;

    save_user_skill_to_root(connection, &skills_root, skill)
}

/** 保存用户自建 skill 到指定 skills 根目录，测试可用临时目录替代 ~/.cici-note/skills。 */
pub fn save_user_skill_to_root(
    connection: &Connection,
    skills_root: &Path,
    skill: AgentSkill,
) -> Result<AgentSkill, String> {
    if skill.source == BUILT_IN_SKILL_SOURCE || is_built_in_skill_id(&skill.id) {
        return Err("内置 skill 不能编辑，只能启用或禁用。".to_owned());
    }

    let normalized_skill = normalize_file_skill_input(skill)?;
    let previous_skill_markdown_path =
        resolve_existing_skill_markdown_path(skills_root, &normalized_skill)?;
    let previous_skill_id = previous_skill_markdown_path
        .as_ref()
        .map(|path| create_file_skill_id(&stable_absolute_path(path)));
    let skill_markdown_path = write_skill_files(
        skills_root,
        &normalized_skill,
        previous_skill_markdown_path.as_deref(),
    )?;
    let mut persisted_skills = read_persisted_skills(connection)?;
    let mut saved_skill =
        load_file_skill(skills_root, &skill_markdown_path, &mut persisted_skills)?;

    saved_skill.enabled = normalized_skill.enabled;
    saved_skill.allow_auto_invoke = normalized_skill.allow_auto_invoke;
    saved_skill.updated_at = format_local_datetime();
    if let Some(previous_skill_id) = previous_skill_id.filter(|id| id != &saved_skill.id) {
        // name 改动会改变 SKILL.md 路径和 file skill ID，需要清理旧路径上的状态覆盖。
        delete_skill_override(connection, &previous_skill_id)?;
    }
    upsert_skill_state_override(connection, &saved_skill)?;

    Ok(saved_skill)
}

/** 启停任意 skill；SQLite 只保存启停和模型参考覆盖，不保存文件式 skill 正文。 */
pub fn toggle_agent_skill(
    app: &AppHandle,
    connection: &Connection,
    skill_id: &str,
    enabled: bool,
    allow_auto_invoke: Option<bool>,
) -> Result<AgentSkill, String> {
    let mut skill = load_agent_skills(app, connection)?
        .into_iter()
        .find(|item| item.id == skill_id)
        .ok_or_else(|| "找不到要更新的 skill。".to_owned())?;

    skill.enabled = enabled;
    if let Some(allow_auto_invoke) = allow_auto_invoke {
        skill.allow_auto_invoke = allow_auto_invoke;
    }
    skill.updated_at = format_local_datetime();
    upsert_skill_state_override(connection, &skill)?;

    Ok(skill)
}

/** 删除用户自建 skill；内置 skill 必须保留供后续重新启用。 */
pub fn delete_user_skill(
    app: &AppHandle,
    connection: &Connection,
    skill_id: &str,
) -> Result<(), String> {
    let skills_root = user_skills_root(app)?;

    delete_user_skill_from_root(connection, &skills_root, skill_id)
}

/** 删除指定 skills 根目录中的用户 skill；文件式 skill 会移除对应目录。 */
pub fn delete_user_skill_from_root(
    connection: &Connection,
    skills_root: &Path,
    skill_id: &str,
) -> Result<(), String> {
    if is_built_in_skill_id(skill_id) {
        return Err("内置 skill 不能删除，请改为禁用。".to_owned());
    }

    let mut persisted_skills = read_persisted_skills(connection)?;
    let file_skill = scan_file_skills(&[skills_root.to_path_buf()], &mut persisted_skills)
        .into_iter()
        .find(|skill| skill.id == skill_id);

    if let Some(skill) = file_skill {
        delete_file_skill_directory(skills_root, &skill)?;
        delete_skill_override(connection, skill_id)?;

        return Ok(());
    }

    let _write_guard = lock_database_writer()?;
    let affected = connection
        .execute(
            "DELETE FROM agent_skills WHERE id = ?1 AND source = ?2",
            params![skill_id, USER_SKILL_SOURCE],
        )
        .map_err(|error| format!("无法删除 skill：{error}"))?;

    if affected == 0 {
        return Err("找不到可删除的用户 skill。".to_owned());
    }

    Ok(())
}

/** 安装来源已经准备成目录后，将其中的标准 SKILL.md 包复制到用户 skills 根目录。 */
pub fn install_agent_skills_from_prepared_root(
    connection: &Connection,
    skills_root: &Path,
    prepared_root: &Path,
    options: SkillInstallOptions,
) -> Result<InstallAgentSkillResult, String> {
    let operation_started_at = format_local_datetime();
    let discovered_skills = discover_installable_skills(prepared_root)?;

    if discovered_skills.is_empty() {
        return Err("安装来源中没有找到有效 SKILL.md。".to_owned());
    }

    let mut warnings = Vec::new();
    let mut installed_skill_paths = Vec::new();
    let mut installed_file_count = 0usize;

    fs::create_dir_all(skills_root).map_err(|error| {
        format!(
            "无法创建用户 Skills 目录 {}：{error}",
            skills_root.display()
        )
    })?;

    for discovered_skill in &discovered_skills {
        validate_install_conflict(
            skills_root,
            &discovered_skill.target_folder_name,
            &options.conflict_strategy,
        )?;
    }

    for discovered_skill in discovered_skills {
        let install_result = install_discovered_skill(
            connection,
            skills_root,
            &discovered_skill,
            &options,
            &operation_started_at,
        )?;

        warnings.extend(install_result.warnings);
        installed_file_count += install_result.file_count;
        installed_skill_paths.push(install_result.skill_markdown_path);
    }

    let mut persisted_skills = read_persisted_skills(connection)?;
    let mut installed_skills = installed_skill_paths
        .iter()
        .map(|skill_path| load_file_skill(skills_root, skill_path, &mut persisted_skills))
        .collect::<Result<Vec<_>, String>>()?;

    installed_skills.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let skills = load_agent_skills_from_roots(connection, &[skills_root.to_path_buf()])?;
    let summary = format!(
        "已安装 {} 个 Skill，复制 {} 个文件。",
        installed_skills.len(),
        installed_file_count
    );

    Ok(InstallAgentSkillResult {
        installed_count: installed_skills.len(),
        installed_skills,
        skills,
        warnings,
        summary,
        source_type: options.source_type,
        source_summary: options.source_summary,
        file_count: installed_file_count,
    })
}

/** 把单个远程 SKILL.md 内容写入临时目录，供统一安装管线复用。 */
pub fn prepare_single_skill_markdown(content: &str) -> Result<TempDir, String> {
    if content.len() > MAX_REMOTE_SKILL_MARKDOWN_BYTES {
        return Err("远程 SKILL.md 超过 1MB，已阻止安装。".to_owned());
    }

    let parsed_skill = parse_skill_markdown(content)?;
    let temp_dir = TempDir::new().map_err(|error| format!("无法创建安装临时目录：{error}"))?;
    let skill_dir = temp_dir
        .path()
        .join(safe_skill_folder_name(&normalize_skill_name(
            &parsed_skill.name,
        ))?);

    fs::create_dir_all(&skill_dir).map_err(|error| format!("无法创建临时 skill 目录：{error}"))?;
    fs::write(skill_dir.join(SKILL_MARKDOWN_FILE_NAME), content)
        .map_err(|error| format!("无法写入临时 SKILL.md：{error}"))?;

    Ok(temp_dir)
}

/** 把 zip 字节安全解压到临时目录，拒绝路径穿越、过大文件和过多文件。 */
pub fn prepare_skill_archive_bytes(bytes: &[u8]) -> Result<TempDir, String> {
    if bytes.len() > MAX_REMOTE_SKILL_ARCHIVE_BYTES {
        return Err("远程 Skill 压缩包超过 25MB，已阻止安装。".to_owned());
    }

    let temp_dir = TempDir::new().map_err(|error| format!("无法创建安装临时目录：{error}"))?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("无法读取 Skill zip 压缩包：{error}"))?;
    let mut extracted_file_count = 0usize;
    let mut extracted_total_bytes = 0u64;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| format!("无法读取 Skill zip 条目：{error}"))?;
        let enclosed_path = file
            .enclosed_name()
            .ok_or_else(|| "Skill zip 包含不安全路径，已阻止安装。".to_owned())?
            .to_path_buf();

        if should_skip_install_relative_path(&enclosed_path) {
            continue;
        }

        let target_path = temp_dir.path().join(&enclosed_path);

        if file.is_dir() {
            fs::create_dir_all(&target_path)
                .map_err(|error| format!("无法创建临时解压目录：{error}"))?;
            continue;
        }

        extracted_file_count += 1;
        if extracted_file_count > MAX_SKILL_INSTALL_FILE_COUNT {
            return Err("Skill 包文件数量超过限制，已阻止安装。".to_owned());
        }

        let file_size = file.size();

        if file_size > MAX_SKILL_INSTALL_SINGLE_FILE_BYTES {
            return Err("Skill 包包含超过 5MB 的单个文件，已阻止安装。".to_owned());
        }

        extracted_total_bytes = extracted_total_bytes
            .checked_add(file_size)
            .ok_or_else(|| "Skill 包总大小超过限制，已阻止安装。".to_owned())?;

        if extracted_total_bytes > MAX_SKILL_INSTALL_TOTAL_BYTES {
            return Err("Skill 包解压后超过 50MB，已阻止安装。".to_owned());
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("无法创建临时解压目录：{error}"))?;
        }

        let mut output_file = fs::File::create(&target_path)
            .map_err(|error| format!("无法创建临时解压文件：{error}"))?;

        std::io::copy(&mut file, &mut output_file)
            .map_err(|error| format!("无法写入临时解压文件：{error}"))?;
    }

    Ok(temp_dir)
}

/** 根据用户显式选择决定本轮激活的完整 skill；未选择时由模型参考能力目录自主判断。 */
pub fn resolve_active_skill(
    skills: &[AgentSkill],
    _settings: &UserSettings,
    request: &AgentTurnRequest,
) -> Option<AgentSkill> {
    if let Some(selected_skill_id) = request.selected_skill_id.as_deref() {
        let selected_skill = skills
            .iter()
            .find(|skill| skill.enabled && skill.id == selected_skill_id)
            .cloned();

        if selected_skill.is_none() {
            log::warn!(
                target: "agent_skill",
                "显式选择的 Skill 不可用或已禁用：skill_id={selected_skill_id}"
            );
        }

        return selected_skill;
    }

    log::debug!(
        target: "agent_skill",
        "未显式选择 Skill，Runtime 不做关键词路由：action={} enabled_skill_count={}",
        request.action,
        skills.iter().filter(|skill| skill.enabled).count()
    );

    None
}

/** 生成注入模型 system prompt 的可用 skill 目录，由模型按语义自主参考而非宿主关键词匹配。 */
pub fn skill_catalog_prompt(skills: &[AgentSkill], settings: &SkillSettings) -> String {
    if settings.activation_mode != "auto" {
        return "可用 Skills：自动参考已关闭；只有用户显式选择的 Skill 会注入完整执行说明。"
            .to_owned();
    }

    let enabled_summaries = skills
        .iter()
        .filter(|skill| skill.enabled && skill.allow_auto_invoke)
        .map(|skill| {
            let path_summary = skill
                .path
                .as_deref()
                .map(|path| format!("；路径：{path}"))
                .unwrap_or_default();

            format!(
                "- {} (`{}`): {}{}；执行要求：{}；语义线索：{}",
                skill.display_name,
                skill.name,
                skill.description,
                path_summary,
                truncate_chars(&skill.instructions, 900),
                skill.triggers.join("、")
            )
        })
        .collect::<Vec<_>>();

    if enabled_summaries.is_empty() {
        "可用 Skills：没有允许自动参考的已启用 Skill。".to_owned()
    } else {
        truncate_chars(
            &format!(
                "可用 Skills（模型按语义自主参考；宿主不会按关键词强制激活）：\n{}",
                enabled_summaries.join("\n")
            ),
            MAX_SKILL_CATALOG_PROMPT_CHARS,
        )
    }
}

/** 生成本轮显式激活 skill 的完整说明，未选择时不再由宿主关键词命中。 */
pub fn active_skill_prompt(active_skill: Option<&AgentSkill>) -> String {
    active_skill
        .map(|skill| {
            format!(
                "本轮激活 Skill：{} (`{}`)\n说明：{}\n执行要求：{}",
                skill.display_name, skill.name, skill.description, skill.instructions
            )
        })
        .unwrap_or_else(|| "本轮未激活 Skill；按普通 Agent 工具边界处理。".to_owned())
}

/** 构造 UI 和审计日志可见的 skill 激活轨迹。 */
pub fn skill_summary(active_skill: Option<&AgentSkill>) -> String {
    active_skill
        .map(|skill| {
            skill
                .path
                .as_deref()
                .map(|path| format!("已激活 Skill：{}（{}）", skill.display_name, path))
                .unwrap_or_else(|| format!("已激活 Skill：{}", skill.display_name))
        })
        .unwrap_or_else(|| "未显式选择 Skill；模型按能力目录和工具边界自主判断".to_owned())
}

/** 创建一条内置 skill，统一填充稳定元数据。 */
fn built_in_skill(
    id: &str,
    name: &str,
    display_name: &str,
    description: &str,
    instructions: &str,
    tags: &[&str],
    triggers: &[&str],
) -> AgentSkill {
    AgentSkill {
        id: id.to_owned(),
        name: name.to_owned(),
        display_name: display_name.to_owned(),
        description: description.to_owned(),
        instructions: instructions.to_owned(),
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        triggers: triggers
            .iter()
            .map(|trigger| (*trigger).to_owned())
            .collect(),
        enabled: true,
        source: BUILT_IN_SKILL_SOURCE.to_owned(),
        allow_auto_invoke: true,
        created_at: "内置".to_owned(),
        updated_at: "内置".to_owned(),
        path: None,
        relative_path: None,
        metadata: None,
    }
}

/** 解析用户目录中的文件式 skills；无效 SKILL.md 只跳过并写日志，不阻塞其他 skill。 */
fn scan_file_skills(
    roots: &[PathBuf],
    persisted_skills: &mut HashMap<String, AgentSkill>,
) -> Vec<AgentSkill> {
    let mut skills = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_walk_skill_entry(entry))
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() != SKILL_MARKDOWN_FILE_NAME {
                continue;
            }

            // 单个 SKILL.md 解析失败不能影响同目录下其他文件式 skill 的加载。
            match load_file_skill(root, entry.path(), persisted_skills) {
                Ok(skill) => skills.push(skill),
                Err(error) => {
                    log::warn!(
                        target: "skill",
                        "跳过无效文件 skill {}：{error}",
                        entry.path().display()
                    );
                }
            }
        }
    }

    skills
}

/** 判断文件 skill 扫描是否继续进入目录，避免递归隐藏目录和常见依赖目录。 */
fn should_walk_skill_entry(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }

    let Some(name) = entry.file_name().to_str() else {
        return true;
    };

    !name.starts_with('.') && !matches!(name, "node_modules" | "target" | "dist" | "build")
}

/** 从单个 SKILL.md 读取文件式 skill，并合并 SQLite 中的启停覆盖。 */
fn load_file_skill(
    root: &Path,
    skill_markdown_path: &Path,
    persisted_skills: &mut HashMap<String, AgentSkill>,
) -> Result<AgentSkill, String> {
    let absolute_root = stable_absolute_path(root);
    let absolute_path = stable_absolute_path(skill_markdown_path);
    let content = fs::read_to_string(skill_markdown_path)
        .map_err(|error| format!("无法读取 SKILL.md：{error}"))?;
    let parsed_skill = parse_skill_markdown(&content)?;
    let metadata = read_openai_yaml_metadata(
        skill_markdown_path
            .parent()
            .ok_or_else(|| "无法解析 skill 目录。".to_owned())?,
    );
    let relative_path = absolute_path
        .strip_prefix(&absolute_root)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| absolute_path.to_string_lossy().to_string());
    let mut metadata_map = HashMap::new();

    metadata_map.insert("frontmatterName".to_owned(), parsed_skill.name.clone());
    if parsed_skill.display_name.is_some() {
        metadata_map.insert(
            "displayNameSource".to_owned(),
            "SKILL.md frontmatter".to_owned(),
        );
    }
    if metadata.display_name_override.is_some() {
        metadata_map.insert(
            "displayNameSource".to_owned(),
            "agents/openai.yaml".to_owned(),
        );
    }
    if metadata.allow_auto_invoke_override.is_some() {
        metadata_map.insert(
            "allowAutoInvokeSource".to_owned(),
            "agents/openai.yaml".to_owned(),
        );
    }

    let mut skill = AgentSkill {
        id: create_file_skill_id(&absolute_path),
        name: normalize_skill_name(&parsed_skill.name),
        display_name: metadata
            .display_name_override
            .or(parsed_skill.display_name)
            .unwrap_or_else(|| parsed_skill.name.clone()),
        description: parsed_skill.description,
        instructions: parsed_skill.instructions,
        tags: normalize_terms(parsed_skill.tags),
        triggers: {
            let normalized_triggers = normalize_terms(parsed_skill.triggers);

            if normalized_triggers.is_empty() {
                derive_file_skill_triggers(&parsed_skill.name)
            } else {
                normalized_triggers
            }
        },
        enabled: true,
        source: FILE_SKILL_SOURCE.to_owned(),
        allow_auto_invoke: metadata.allow_auto_invoke_override.unwrap_or(true),
        created_at: "文件".to_owned(),
        updated_at: format_local_datetime(),
        path: Some(absolute_path.to_string_lossy().to_string()),
        relative_path: Some(relative_path),
        metadata: Some(metadata_map),
    };

    if skill.name.is_empty() {
        skill.name = parsed_skill.name.trim().to_lowercase();
    }

    if let Some(saved_skill) = persisted_skills.remove(&skill.id) {
        // 文件正文永远以磁盘为准，只继承用户在 UI 中保存的状态覆盖。
        skill.enabled = saved_skill.enabled;
        skill.allow_auto_invoke = saved_skill.allow_auto_invoke;
        skill.updated_at = saved_skill.updated_at;
    }

    Ok(skill)
}

/** 文件式 skill 的 frontmatter 解析结果，正文即完整执行说明。 */
#[derive(Clone, Debug)]
struct ParsedSkillMarkdown {
    name: String,
    display_name: Option<String>,
    description: String,
    instructions: String,
    tags: Vec<String>,
    triggers: Vec<String>,
}

/** agents/openai.yaml 中首版支持的 UI 与策略覆盖字段。 */
#[derive(Default)]
struct OpenAiSkillMetadata {
    display_name_override: Option<String>,
    allow_auto_invoke_override: Option<bool>,
}

/** 安装管线的显式选项，调用方负责把 URL、本地目录或压缩包准备成目录。 */
#[derive(Clone, Debug)]
pub struct SkillInstallOptions {
    /** 来源类型只进入脱敏日志和前端摘要，不参与文件系统路径判断。 */
    pub source_type: String,
    /** 来源摘要必须由调用方脱敏，不能包含完整 URL 或本机绝对路径。 */
    pub source_summary: String,
    /** 第三方 skill 默认停用，用户审阅后再启用。 */
    pub enable_after_install: bool,
    /** 同名目录冲突处理策略，首版支持 fail 和 replace。 */
    pub conflict_strategy: String,
}

/** 安装前在来源目录中发现的一个 SKILL.md 包。 */
#[derive(Clone, Debug)]
struct DiscoveredInstallableSkill {
    source_dir: PathBuf,
    target_folder_name: String,
    content_hash: String,
}

/** 单个 skill 安装后的文件复制结果。 */
struct InstalledSkillFiles {
    skill_markdown_path: PathBuf,
    file_count: usize,
    warnings: Vec<String>,
}

/** 解析 SKILL.md 的 YAML frontmatter；首版只要求 name 和 description 两个键。 */
fn parse_skill_markdown(content: &str) -> Result<ParsedSkillMarkdown, String> {
    let normalized_content = content.strip_prefix('\u{feff}').unwrap_or(content);

    if !normalized_content.starts_with("---") {
        return Err("缺少 YAML frontmatter。".to_owned());
    }

    let mut lines = normalized_content.lines();
    let first_line = lines.next().unwrap_or_default();

    if first_line.trim() != "---" {
        return Err("frontmatter 起始标记必须是 ---。".to_owned());
    }

    let mut frontmatter_lines = Vec::new();
    let mut body_lines = Vec::new();
    let mut in_frontmatter = true;

    for line in lines {
        if in_frontmatter && line.trim() == "---" {
            in_frontmatter = false;
            continue;
        }

        if in_frontmatter {
            frontmatter_lines.push(line);
        } else {
            body_lines.push(line);
        }
    }

    if in_frontmatter {
        return Err("frontmatter 缺少结束标记 ---。".to_owned());
    }

    let frontmatter_text = frontmatter_lines.join("\n");
    let frontmatter = serde_yaml::from_str::<serde_yaml::Mapping>(&frontmatter_text)
        .map_err(|error| format!("frontmatter 不是有效 YAML：{error}"))?;
    let name = yaml_mapping_string(&frontmatter, "name")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "frontmatter 缺少 name。".to_owned())?;
    let description = yaml_mapping_string(&frontmatter, "description")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "frontmatter 缺少 description。".to_owned())?;
    let display_name = yaml_mapping_string(&frontmatter, "display_name")
        .or_else(|| yaml_mapping_string(&frontmatter, "title"));
    let tags = yaml_mapping_list(&frontmatter, "tags");
    let triggers = yaml_mapping_list(&frontmatter, "triggers");
    let instructions = body_lines.join("\n").trim().to_owned();

    if instructions.is_empty() {
        return Err("SKILL.md 正文不能为空。".to_owned());
    }

    Ok(ParsedSkillMarkdown {
        name,
        display_name,
        description,
        instructions,
        tags,
        triggers,
    })
}

/** 从 YAML mapping 中读取字符串标量字段；非字符串字段会被忽略。 */
fn yaml_mapping_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.to_owned()))
        .and_then(|value| match value {
            serde_yaml::Value::String(text) => Some(text.trim().to_owned()),
            serde_yaml::Value::Number(number) => Some(number.to_string()),
            serde_yaml::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty())
}

/** 从 YAML mapping 中读取字符串列表字段，兼容数组和逗号分隔字符串。 */
fn yaml_mapping_list(mapping: &serde_yaml::Mapping, key: &str) -> Vec<String> {
    let Some(value) = mapping.get(serde_yaml::Value::String(key.to_owned())) else {
        return Vec::new();
    };

    match value {
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|item| match item {
                serde_yaml::Value::String(text) => Some(text.trim().to_owned()),
                serde_yaml::Value::Number(number) => Some(number.to_string()),
                serde_yaml::Value::Bool(value) => Some(value.to_string()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
            .collect(),
        serde_yaml::Value::String(text) => parse_frontmatter_list(text),
        _ => Vec::new(),
    }
}

/** 读取 agents/openai.yaml 的 display_name 和 allow_implicit_invocation 覆盖。 */
fn read_openai_yaml_metadata(skill_dir: &Path) -> OpenAiSkillMetadata {
    let metadata_path = skill_dir.join("agents").join("openai.yaml");
    let Ok(content) = fs::read_to_string(&metadata_path) else {
        return OpenAiSkillMetadata::default();
    };
    let mut metadata = OpenAiSkillMetadata::default();
    let mut current_section = "";

    for raw_line in content.lines() {
        let line = raw_line.trim_end();
        let trimmed_line = line.trim();

        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }

        if !raw_line.starts_with(' ') && trimmed_line.ends_with(':') {
            current_section = trimmed_line.trim_end_matches(':');
            continue;
        }

        if current_section == "interface" && trimmed_line.starts_with("display_name:") {
            metadata.display_name_override = parse_yaml_value_after_colon(trimmed_line);
        }

        if current_section == "policy" && trimmed_line.starts_with("allow_implicit_invocation:") {
            metadata.allow_auto_invoke_override = parse_yaml_value_after_colon(trimmed_line)
                .map(|value| value.to_lowercase() != "false");
        }
    }

    metadata
}

/** 从单行 yaml 字段中提取冒号后的标量值。 */
fn parse_yaml_value_after_colon(line: &str) -> Option<String> {
    line.split_once(':')
        .map(|(_, value)| trim_yaml_scalar(value))
        .filter(|value| !value.is_empty())
}

/** 解析 frontmatter 中的逗号分隔或简单数组字段，用于保留 UI 表单里的标签和语义线索。 */
fn parse_frontmatter_list(value: &str) -> Vec<String> {
    let trimmed_value = value.trim();
    let list_body = trimmed_value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(trimmed_value);

    list_body
        .split(',')
        .map(trim_yaml_scalar)
        .filter(|value| !value.is_empty())
        .collect()
}

/** 清理简单 YAML 标量两侧空白和一层引号；首版不解析数组或多行值。 */
fn trim_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_owned()
}

/** 把 UI 表单提交的 skill 归一化为可写入 SKILL.md 的文件式定义。 */
fn normalize_file_skill_input(mut skill: AgentSkill) -> Result<AgentSkill, String> {
    skill.name = normalize_skill_name(&skill.name);
    skill.display_name = skill.display_name.trim().to_owned();
    skill.description = skill.description.trim().to_owned();
    skill.instructions = skill.instructions.trim().to_owned();
    skill.tags = normalize_terms(skill.tags);
    skill.triggers = normalize_terms(skill.triggers);
    skill.source = FILE_SKILL_SOURCE.to_owned();
    skill.metadata = None;

    if skill.name.is_empty() {
        skill.name = normalize_skill_name(&skill.display_name);
    }

    if skill.name.is_empty() {
        return Err("Skill 标识 name 不能为空。".to_owned());
    }

    if skill.display_name.is_empty() {
        return Err("Skill 名称不能为空。".to_owned());
    }

    if skill.description.is_empty() {
        return Err("Skill 描述不能为空。".to_owned());
    }

    if skill.instructions.is_empty() {
        return Err("Skill 执行说明不能为空。".to_owned());
    }

    Ok(skill)
}

/** 将用户创建或编辑的 skill 写入 ~/.cici-note/skills/<name>/SKILL.md。 */
fn write_skill_files(
    skills_root: &Path,
    skill: &AgentSkill,
    previous_skill_markdown_path: Option<&Path>,
) -> Result<PathBuf, String> {
    let skill_dir = skills_root.join(safe_skill_folder_name(&skill.name)?);
    let skill_markdown_path = skill_dir.join(SKILL_MARKDOWN_FILE_NAME);

    move_previous_skill_directory_if_needed(skills_root, previous_skill_markdown_path, &skill_dir)?;
    fs::create_dir_all(&skill_dir)
        .map_err(|error| format!("无法创建 skill 目录 {}：{error}", skill_dir.display()))?;
    fs::write(&skill_markdown_path, build_skill_markdown(skill)).map_err(|error| {
        format!(
            "无法写入 SKILL.md {}：{error}",
            skill_markdown_path.display()
        )
    })?;
    write_openai_yaml_override(&skill_dir, skill)?;

    Ok(skill_markdown_path)
}

/** 解析编辑前的 SKILL.md 路径，供 name 变更时迁移旧目录和清理旧状态覆盖。 */
fn resolve_existing_skill_markdown_path(
    skills_root: &Path,
    skill: &AgentSkill,
) -> Result<Option<PathBuf>, String> {
    if let Some(relative_path) = skill.relative_path.as_deref() {
        let relative = Path::new(relative_path);

        if relative
            .file_name()
            .is_some_and(|name| name == SKILL_MARKDOWN_FILE_NAME)
        {
            if let Some(parent) = relative
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                return Ok(Some(
                    skills_root
                        .join(safe_relative_skill_folder(parent)?)
                        .join(SKILL_MARKDOWN_FILE_NAME),
                ));
            }
        }
    }

    if let Some(path) = skill.path.as_deref() {
        let absolute_path = Path::new(path);
        let absolute_root = stable_absolute_path(skills_root);
        let absolute_skill_path = stable_absolute_path(absolute_path);

        if absolute_skill_path.starts_with(&absolute_root)
            && absolute_skill_path
                .file_name()
                .is_some_and(|name| name == SKILL_MARKDOWN_FILE_NAME)
        {
            if let Ok(relative) = absolute_skill_path.strip_prefix(&absolute_root) {
                if let Some(parent) = relative
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                {
                    return Ok(Some(
                        skills_root
                            .join(safe_relative_skill_folder(parent)?)
                            .join(SKILL_MARKDOWN_FILE_NAME),
                    ));
                }
            }
        }
    }

    Ok(None)
}

/** 根据 name 生成单级 skill 目录名；name 本身就是目录名，不再从旧路径继承。 */
fn safe_skill_folder_name(name: &str) -> Result<String, String> {
    let path = Path::new(name);

    if path.components().count() != 1 {
        return Err("Skill 标识 name 只能是单级目录名。".to_owned());
    }

    safe_relative_skill_folder(path)
}

/** 编辑时如果 name 改变，需要把旧 skill 目录同步迁移到新目录。 */
fn move_previous_skill_directory_if_needed(
    skills_root: &Path,
    previous_skill_markdown_path: Option<&Path>,
    next_skill_dir: &Path,
) -> Result<(), String> {
    let Some(previous_skill_markdown_path) = previous_skill_markdown_path else {
        return Ok(());
    };
    let previous_skill_dir = previous_skill_markdown_path
        .parent()
        .ok_or_else(|| "无法解析旧 skill 目录。".to_owned())?;
    let absolute_root = stable_absolute_path(skills_root);
    let absolute_previous_dir = stable_absolute_path(previous_skill_dir);
    let absolute_next_dir = stable_absolute_path(next_skill_dir);

    if absolute_previous_dir == absolute_next_dir {
        return Ok(());
    }

    if !absolute_previous_dir.starts_with(&absolute_root) {
        return Err("只能迁移 Cici Note 用户 Skills 目录内的 skill。".to_owned());
    }

    if next_skill_dir.exists() {
        return Err("目标 Skill 目录已存在，请换一个 name。".to_owned());
    }

    if previous_skill_dir.exists() {
        // 目录迁移保留用户可能放在 skill 文件夹中的附加资料，但不执行这些资料。
        fs::rename(previous_skill_dir, next_skill_dir).map_err(|error| {
            format!(
                "无法迁移 skill 目录 {} 到 {}：{error}",
                previous_skill_dir.display(),
                next_skill_dir.display()
            )
        })?;
    }

    Ok(())
}

/** 确保 skill 目录相对路径不含上级目录、绝对路径或隐藏目录。 */
fn safe_relative_skill_folder(relative_path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();

    for component in relative_path.components() {
        let Component::Normal(part) = component else {
            return Err("Skill 目录名无效。".to_owned());
        };
        let part = part.to_string_lossy().to_string();

        // 文件式 skill 目录只允许普通相对组件，避免相对路径覆盖用户目录外内容。
        if part.is_empty() || part == "." || part == ".." || part.starts_with('.') {
            return Err("Skill 目录名无效。".to_owned());
        }

        parts.push(part);
    }

    if parts.is_empty() {
        return Err("Skill 目录名无效。".to_owned());
    }

    Ok(parts.join("/"))
}

/** 构造写入磁盘的 SKILL.md，保留 tags/triggers 作为 Cici Note frontmatter 扩展字段。 */
fn build_skill_markdown(skill: &AgentSkill) -> String {
    let mut frontmatter = vec![
        ("name", yaml_quote(&skill.name)),
        ("description", yaml_quote(&skill.description)),
    ];

    if !skill.tags.is_empty() {
        frontmatter.push(("tags", yaml_array(&skill.tags)));
    }
    if !skill.triggers.is_empty() {
        frontmatter.push(("triggers", yaml_array(&skill.triggers)));
    }

    let frontmatter_text = frontmatter
        .into_iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "---\n{frontmatter_text}\n---\n\n{}\n",
        skill.instructions.trim()
    )
}

/** displayName 和模型参考偏好写到 agents/openai.yaml，保持 SKILL.md 兼容 Codex 风格。 */
fn write_openai_yaml_override(skill_dir: &Path, skill: &AgentSkill) -> Result<(), String> {
    let agents_dir = skill_dir.join("agents");
    let metadata_path = agents_dir.join("openai.yaml");
    let content = format!(
        "interface:\n  display_name: {}\npolicy:\n  allow_implicit_invocation: {}\n",
        yaml_quote(&skill.display_name),
        if skill.allow_auto_invoke {
            "true"
        } else {
            "false"
        }
    );

    fs::create_dir_all(&agents_dir).map_err(|error| {
        format!(
            "无法创建 skill 元数据目录 {}：{error}",
            agents_dir.display()
        )
    })?;
    fs::write(&metadata_path, content)
        .map_err(|error| format!("无法写入 skill 元数据 {}：{error}", metadata_path.display()))
}

/** 递归发现待安装目录中的 SKILL.md，并在安装前完成格式校验。 */
fn discover_installable_skills(
    prepared_root: &Path,
) -> Result<Vec<DiscoveredInstallableSkill>, String> {
    if !prepared_root.exists() || !prepared_root.is_dir() {
        return Err("安装来源目录不存在。".to_owned());
    }

    let mut skills = Vec::new();

    for entry in WalkDir::new(prepared_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_walk_skill_entry(entry))
    {
        let entry = entry.map_err(|error| format!("无法读取安装来源目录：{error}"))?;

        if !entry.file_type().is_file() || entry.file_name() != SKILL_MARKDOWN_FILE_NAME {
            continue;
        }

        let content = fs::read_to_string(entry.path())
            .map_err(|error| format!("无法读取待安装 SKILL.md：{error}"))?;
        let parsed_skill = parse_skill_markdown(&content)?;
        let normalized_name = normalize_skill_name(&parsed_skill.name);
        let content_hash = hash_skill_directory(entry.path().parent().unwrap_or(prepared_root))?;
        let target_folder_name = if normalized_name.is_empty() {
            format!("skill-{}", &content_hash[..12])
        } else {
            safe_skill_folder_name(&normalized_name)?
        };
        let source_dir = entry
            .path()
            .parent()
            .ok_or_else(|| "无法解析待安装 skill 目录。".to_owned())?
            .to_path_buf();

        skills.push(DiscoveredInstallableSkill {
            source_dir,
            target_folder_name,
            content_hash,
        });
    }

    if has_duplicate_install_targets(&skills) {
        return Err("安装包中存在重复的 skill name，请拆分或改名后重试。".to_owned());
    }

    skills.sort_by(|left, right| left.target_folder_name.cmp(&right.target_folder_name));

    Ok(skills)
}

/** 判断同一安装批次内是否会写入同一个目标目录。 */
fn has_duplicate_install_targets(skills: &[DiscoveredInstallableSkill]) -> bool {
    let mut targets = HashSet::new();

    skills
        .iter()
        .any(|skill| !targets.insert(skill.target_folder_name.clone()))
}

/** 安装前校验目标目录冲突策略，避免安装到一半才发现不可覆盖。 */
fn validate_install_conflict(
    skills_root: &Path,
    target_folder_name: &str,
    conflict_strategy: &str,
) -> Result<(), String> {
    let target_dir = skills_root.join(target_folder_name);

    if !target_dir.exists() {
        return Ok(());
    }

    if conflict_strategy == INSTALL_CONFLICT_REPLACE {
        return Ok(());
    }

    if conflict_strategy == INSTALL_CONFLICT_FAIL {
        return Err(format!(
            "Skill「{target_folder_name}」已存在，请开启替换同名 Skill 后重试。"
        ));
    }

    Err("未知的 Skill 安装冲突处理策略。".to_owned())
}

/** 安装单个已发现 skill；先写 staging 目录，成功后再替换目标目录。 */
fn install_discovered_skill(
    connection: &Connection,
    skills_root: &Path,
    discovered_skill: &DiscoveredInstallableSkill,
    options: &SkillInstallOptions,
    installed_at: &str,
) -> Result<InstalledSkillFiles, String> {
    let target_dir = skills_root.join(&discovered_skill.target_folder_name);
    let staging_dir = skills_root.join(format!(
        ".installing-{}-{}",
        discovered_skill.target_folder_name,
        create_id("skill")
    ));
    let mut warnings = Vec::new();
    let file_count =
        copy_skill_directory_checked(&discovered_skill.source_dir, &staging_dir, &mut warnings)?;

    write_cici_install_metadata(
        &staging_dir,
        discovered_skill,
        options,
        installed_at,
        file_count,
    )?;

    if target_dir.exists() {
        if options.conflict_strategy != INSTALL_CONFLICT_REPLACE {
            let _ = fs::remove_dir_all(&staging_dir);
            return Err(format!(
                "Skill「{}」已存在，请开启替换同名 Skill 后重试。",
                discovered_skill.target_folder_name
            ));
        }

        fs::remove_dir_all(&target_dir)
            .map_err(|error| format!("无法替换已有 Skill 目录：{error}"))?;
    }

    fs::rename(&staging_dir, &target_dir).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_dir);
        format!("无法安装 Skill 到用户目录：{error}")
    })?;

    let skill_markdown_path = target_dir.join(SKILL_MARKDOWN_FILE_NAME);
    let mut persisted_skills = read_persisted_skills(connection)?;
    let mut installed_skill =
        load_file_skill(skills_root, &skill_markdown_path, &mut persisted_skills)?;

    installed_skill.enabled = options.enable_after_install;
    installed_skill.allow_auto_invoke = options.enable_after_install;
    installed_skill.updated_at = format_local_datetime();
    upsert_skill_state_override(connection, &installed_skill)?;

    Ok(InstalledSkillFiles {
        skill_markdown_path,
        file_count,
        warnings,
    })
}

/** 复制 skill 目录，限制大小、数量和路径，保留 references/assets/scripts 等附带资料。 */
fn copy_skill_directory_checked(
    source_dir: &Path,
    target_dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<usize, String> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;

    fs::create_dir_all(target_dir)
        .map_err(|error| format!("无法创建安装 staging 目录：{error}"))?;

    for entry in WalkDir::new(source_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_walk_skill_entry(entry))
    {
        let entry = entry.map_err(|error| format!("无法读取待安装 skill 文件：{error}"))?;
        let relative_path = entry
            .path()
            .strip_prefix(source_dir)
            .map_err(|_| "无法解析待安装 skill 相对路径。".to_owned())?;

        if relative_path.as_os_str().is_empty() {
            continue;
        }

        if should_skip_install_relative_path(relative_path) {
            if entry.file_type().is_dir() {
                continue;
            }

            continue;
        }

        let target_path = target_dir.join(relative_path);
        let file_type = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("无法读取待安装 skill 文件元数据：{error}"))?
            .file_type();

        if file_type.is_symlink() {
            warnings.push("安装包包含符号链接，已跳过。".to_owned());
            continue;
        }

        if file_type.is_dir() {
            if relative_path
                .components()
                .any(|component| component.as_os_str() == "scripts")
            {
                warnings.push(
                    "安装包包含 scripts 目录；Cici Note 已保留文件但不会执行脚本。".to_owned(),
                );
            }

            fs::create_dir_all(&target_path)
                .map_err(|error| format!("无法创建 skill 子目录：{error}"))?;
            continue;
        }

        if !file_type.is_file() {
            warnings.push("安装包包含非常规文件，已跳过。".to_owned());
            continue;
        }

        file_count += 1;
        if file_count > MAX_SKILL_INSTALL_FILE_COUNT {
            return Err("Skill 包文件数量超过限制，已阻止安装。".to_owned());
        }

        let metadata = entry
            .metadata()
            .map_err(|error| format!("无法读取待安装 skill 文件大小：{error}"))?;

        if metadata.len() > MAX_SKILL_INSTALL_SINGLE_FILE_BYTES {
            return Err("Skill 包包含超过 5MB 的单个文件，已阻止安装。".to_owned());
        }

        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| "Skill 包总大小超过限制，已阻止安装。".to_owned())?;

        if total_bytes > MAX_SKILL_INSTALL_TOTAL_BYTES {
            return Err("Skill 包总大小超过 50MB，已阻止安装。".to_owned());
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("无法创建 skill 子目录：{error}"))?;
        }

        fs::copy(entry.path(), &target_path)
            .map_err(|error| format!("无法复制 skill 文件：{error}"))?;
    }

    Ok(file_count)
}

/** 判断安装包中的相对路径是否应该跳过，避免复制依赖、构建产物和隐藏目录。 */
fn should_skip_install_relative_path(relative_path: &Path) -> bool {
    relative_path.components().any(|component| {
        let Component::Normal(part) = component else {
            return true;
        };
        let name = part.to_string_lossy();

        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | "target" | "dist" | "build" | ".git"
            )
    })
}

/** 写入 Cici Note 安装元数据，保留可审计摘要但不保存完整来源 URL 或绝对路径。 */
fn write_cici_install_metadata(
    skill_dir: &Path,
    discovered_skill: &DiscoveredInstallableSkill,
    options: &SkillInstallOptions,
    installed_at: &str,
    file_count: usize,
) -> Result<(), String> {
    let agents_dir = skill_dir.join("agents");
    let metadata_path = agents_dir.join(CICI_INSTALL_METADATA_FILE_NAME);
    let content = format!(
        "install:\n  source_type: {}\n  source_summary: {}\n  installed_at: {}\n  content_hash: {}\n  file_count: {}\n  default_enabled: {}\n",
        yaml_quote(&options.source_type),
        yaml_quote(&options.source_summary),
        yaml_quote(installed_at),
        yaml_quote(&discovered_skill.content_hash),
        file_count,
        if options.enable_after_install { "true" } else { "false" }
    );

    fs::create_dir_all(&agents_dir)
        .map_err(|error| format!("无法创建 skill 安装元数据目录：{error}"))?;
    fs::write(&metadata_path, content)
        .map_err(|error| format!("无法写入 skill 安装元数据：{error}"))
}

/** 删除用户 skills 根目录中的单个文件式 skill 目录，并限制只能删除根目录内路径。 */
fn delete_file_skill_directory(skills_root: &Path, skill: &AgentSkill) -> Result<(), String> {
    let path = skill
        .path
        .as_deref()
        .ok_or_else(|| "文件 skill 缺少路径，无法删除。".to_owned())?;
    let skill_markdown_path = stable_absolute_path(Path::new(path));
    let absolute_root = stable_absolute_path(skills_root);

    if !skill_markdown_path.starts_with(&absolute_root)
        || skill_markdown_path
            .file_name()
            .is_none_or(|name| name != SKILL_MARKDOWN_FILE_NAME)
    {
        return Err("只能删除 Cici Note 用户 Skills 目录内的 SKILL.md。".to_owned());
    }

    let skill_dir = skill_markdown_path
        .parent()
        .ok_or_else(|| "无法解析 skill 目录。".to_owned())?;

    fs::remove_dir_all(skill_dir)
        .map_err(|error| format!("无法删除 skill 目录 {}：{error}", skill_dir.display()))
}

/** 删除 SQLite 中的 skill 状态覆盖；文件正文不存数据库。 */
fn delete_skill_override(connection: &Connection, skill_id: &str) -> Result<(), String> {
    let _write_guard = lock_database_writer()?;

    connection
        .execute("DELETE FROM agent_skills WHERE id = ?1", params![skill_id])
        .map_err(|error| format!("无法清理 skill 状态覆盖：{error}"))?;

    Ok(())
}

/** 把历史版本保存在 SQLite 的 user skill 写回用户 skills 目录。 */
fn migrate_legacy_user_skills(
    connection: &Connection,
    skills_root: &Path,
    persisted_skills: &mut HashMap<String, AgentSkill>,
) {
    let legacy_skill_ids = persisted_skills
        .values()
        .filter(|skill| skill.source == USER_SKILL_SOURCE)
        .map(|skill| skill.id.clone())
        .collect::<Vec<_>>();

    for legacy_skill_id in legacy_skill_ids {
        let Some(legacy_skill) = persisted_skills.remove(&legacy_skill_id) else {
            continue;
        };

        match save_user_skill_to_root(connection, skills_root, legacy_skill) {
            Ok(saved_skill) => {
                // 当前 load 流程已经读过 SQLite，需要把新文件 skill 的状态覆盖补回内存映射。
                let override_skill = skill_state_override_payload(&saved_skill);
                persisted_skills.insert(override_skill.id.clone(), override_skill);
                if let Err(error) = delete_skill_override(connection, &legacy_skill_id) {
                    log::warn!(target: "skill", "迁移旧版 user skill 后清理 SQLite 失败：{error}");
                }
            }
            Err(error) => {
                log::warn!(target: "skill", "迁移旧版 user skill 失败：{error}");
            }
        }
    }
}

/** 生成简单 YAML 字符串标量，避免冒号、引号或中文破坏 frontmatter。 */
fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/** 生成简单 YAML 数组标量，首版只支持字符串数组。 */
fn yaml_array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| yaml_quote(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/** 文件 skill ID 使用绝对 SKILL.md 路径 hash，文件内容变化不会改变用户覆盖状态。 */
fn create_file_skill_id(skill_markdown_path: &Path) -> String {
    let mut hasher = Sha256::new();

    hasher.update(skill_markdown_path.to_string_lossy().as_bytes());

    let digest = format!("{:x}", hasher.finalize());

    format!("skill-file-{}", &digest[..24])
}

/** 计算 skill 目录内容 hash，安装日志和元数据只记录摘要，不记录正文。 */
fn hash_skill_directory(skill_dir: &Path) -> Result<String, String> {
    let mut files = Vec::new();

    for entry in WalkDir::new(skill_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_walk_skill_entry(entry))
    {
        let entry = entry.map_err(|error| format!("无法读取 skill 目录用于 hash：{error}"))?;

        if !entry.file_type().is_file() {
            continue;
        }

        let relative_path = entry
            .path()
            .strip_prefix(skill_dir)
            .map_err(|_| "无法解析 skill hash 相对路径。".to_owned())?
            .to_path_buf();

        if should_skip_install_relative_path(&relative_path) {
            continue;
        }

        files.push((relative_path, entry.path().to_path_buf()));
    }

    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();

    for (relative_path, path) in files {
        let bytes =
            fs::read(&path).map_err(|error| format!("无法读取 skill 文件用于 hash：{error}"))?;

        hasher.update(relative_path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
        hasher.update([0]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/** 按字符数截断字符串，避免 skill 摘要目录挤占过多模型上下文。 */
fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let truncated = value.chars().take(max_chars).collect::<String>();

    format!("{truncated}\n...（可用 Skills 过多，已截断目录摘要）")
}

/** 根据用户输入的 URL 推导下载目标和脱敏来源摘要。 */
pub fn resolve_skill_url_download(input: &str) -> Result<SkillUrlDownload, String> {
    let trimmed_input = input.trim();

    if trimmed_input.is_empty() {
        return Err("请输入 Skill URL。".to_owned());
    }

    let parsed_url = reqwest::Url::parse(trimmed_input)
        .map_err(|_| "Skill URL 格式无效，请使用 https 地址。".to_owned())?;

    if parsed_url.scheme() != "https" {
        return Err("只支持 https Skill URL。".to_owned());
    }

    let host = parsed_url
        .host_str()
        .ok_or_else(|| "Skill URL 缺少 host。".to_owned())?
        .to_owned();

    if host == "github.com" {
        return resolve_github_skill_url(&parsed_url);
    }

    Ok(SkillUrlDownload {
        url: parsed_url.to_string(),
        kind: SkillUrlDownloadKind::Unknown,
        source_summary: host,
    })
}

/** URL 下载类型决定后续按 SKILL.md 文本还是 zip 字节处理。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillUrlDownloadKind {
    Markdown,
    Archive,
    Unknown,
}

/** 已解析的远程 skill 下载目标，source_summary 只能用于日志和 UI。 */
#[derive(Clone, Debug)]
pub struct SkillUrlDownload {
    pub url: String,
    pub kind: SkillUrlDownloadKind,
    pub source_summary: String,
}

/** 把 GitHub repo/blob/tree 链接转换成 raw SKILL.md 或 zipball 下载地址。 */
fn resolve_github_skill_url(url: &reqwest::Url) -> Result<SkillUrlDownload, String> {
    let parts = url
        .path_segments()
        .ok_or_else(|| "GitHub URL 路径无效。".to_owned())?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if parts.len() < 2 {
        return Err("GitHub URL 至少需要 owner/repo。".to_owned());
    }

    let owner = parts[0];
    let repo = normalize_github_repo_name(parts[1])?;
    let source_summary = format!("github.com/{owner}/{repo}");

    if parts.get(2) == Some(&"blob") && parts.len() >= 5 {
        let branch = parts[3];
        let file_path = parts[4..].join("/");

        if !file_path.ends_with(SKILL_MARKDOWN_FILE_NAME) {
            return Err("GitHub blob 链接必须指向 SKILL.md。".to_owned());
        }

        return Ok(SkillUrlDownload {
            url: format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{file_path}"),
            kind: SkillUrlDownloadKind::Markdown,
            source_summary,
        });
    }

    if parts.get(2) == Some(&"tree") && parts.len() >= 4 {
        let branch = parts[3];

        return Ok(SkillUrlDownload {
            url: format!("https://github.com/{owner}/{repo}/archive/refs/heads/{branch}.zip"),
            kind: SkillUrlDownloadKind::Archive,
            source_summary,
        });
    }

    Ok(SkillUrlDownload {
        url: format!("https://github.com/{owner}/{repo}/archive/refs/heads/main.zip"),
        kind: SkillUrlDownloadKind::Archive,
        source_summary,
    })
}

/** 归一化 GitHub repo 路径片段，兼容用户从 clone 按钮复制的 owner/repo.git URL。 */
fn normalize_github_repo_name(repo: &str) -> Result<String, String> {
    let repo_name = repo.trim_end_matches(".git");

    if repo_name.is_empty() || repo_name.contains('/') {
        return Err("GitHub repo 名称无效。".to_owned());
    }

    Ok(repo_name.to_owned())
}

/** 给文件 skill 派生基础语义线索，仅用于展示和模型能力目录，不参与宿主关键词路由。 */
fn derive_file_skill_triggers(name: &str) -> Vec<String> {
    normalize_terms(vec![name.to_owned(), normalize_skill_name(name)])
}

/** 获取稳定绝对路径；canonicalize 失败时仍尽量基于当前路径生成稳定 ID。 */
fn stable_absolute_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

/** 获取 Cici Note 用户目录，优先 ~/.cici-note，无法读取 home 时回退 app data。 */
fn user_cici_home(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(home_dir) = home_dir() {
        return Ok(home_dir.join(CICI_HOME_DIRECTORY_NAME));
    }

    app.path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))
}

/** 跨平台读取用户 home 目录，避免为单个目录引入额外依赖。 */
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/** 从 SQLite 读取已持久化的 skill payload。 */
fn read_persisted_skills(connection: &Connection) -> Result<HashMap<String, AgentSkill>, String> {
    let mut statement = connection
        .prepare("SELECT payload_json FROM agent_skills ORDER BY updated_at DESC")
        .map_err(|error| format!("无法读取 skill 表：{error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("无法查询 skill：{error}"))?;
    let mut skills = HashMap::new();

    for row in rows {
        let payload_json = row.map_err(|error| format!("无法读取 skill payload：{error}"))?;
        let skill: AgentSkill = serde_json::from_str(&payload_json)
            .map_err(|error| format!("无法解析 skill payload：{error}"))?;

        skills.insert(skill.id.clone(), skill);
    }

    Ok(skills)
}

/** 写入或更新 skill payload；仅用于旧版 user skill 兼容迁移，不作为文件 skill 正文来源。 */
fn upsert_skill(connection: &Connection, skill: &AgentSkill) -> Result<(), String> {
    let payload_json =
        serde_json::to_string(skill).map_err(|error| format!("无法序列化 skill：{error}"))?;
    let _write_guard = lock_database_writer()?;

    connection
        .execute(
            "INSERT OR REPLACE INTO agent_skills (id, source, payload_json, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![skill.id, skill.source, payload_json, skill.updated_at],
        )
        .map_err(|error| format!("无法保存 skill：{error}"))?;

    Ok(())
}

/** 写入 skill 状态覆盖，避免把用户目录中的 SKILL.md 完整复制进 SQLite。 */
fn upsert_skill_state_override(connection: &Connection, skill: &AgentSkill) -> Result<(), String> {
    let override_skill = skill_state_override_payload(skill);

    upsert_skill(connection, &override_skill)
}

/** 构造最小状态覆盖 payload，读取时只使用 enabled、allowAutoInvoke 和 updatedAt。 */
fn skill_state_override_payload(skill: &AgentSkill) -> AgentSkill {
    AgentSkill {
        id: skill.id.clone(),
        name: String::new(),
        display_name: String::new(),
        description: String::new(),
        instructions: String::new(),
        tags: Vec::new(),
        triggers: Vec::new(),
        enabled: skill.enabled,
        source: skill.source.clone(),
        allow_auto_invoke: skill.allow_auto_invoke,
        created_at: String::new(),
        updated_at: skill.updated_at.clone(),
        path: None,
        relative_path: None,
        metadata: None,
    }
}

/** 归一化用户 skill，避免空字段进入 Runtime prompt。 */
fn normalize_user_skill(mut skill: AgentSkill) -> Result<AgentSkill, String> {
    skill.name = normalize_skill_name(&skill.name);
    skill.display_name = skill.display_name.trim().to_owned();
    skill.description = skill.description.trim().to_owned();
    skill.instructions = skill.instructions.trim().to_owned();
    skill.tags = normalize_terms(skill.tags);
    skill.triggers = normalize_terms(skill.triggers);
    skill.source = USER_SKILL_SOURCE.to_owned();
    skill.path = None;
    skill.relative_path = None;
    skill.metadata = None;

    if skill.id.trim().is_empty() {
        skill.id = create_id("skill");
    }

    if skill.name.is_empty() {
        skill.name = skill.id.clone();
    }

    if skill.display_name.is_empty() {
        return Err("Skill 名称不能为空。".to_owned());
    }

    if skill.description.is_empty() {
        return Err("Skill 描述不能为空。".to_owned());
    }

    if skill.instructions.is_empty() {
        return Err("Skill 执行说明不能为空。".to_owned());
    }

    if skill.created_at.trim().is_empty() {
        skill.created_at = format_local_datetime();
    }

    if skill.updated_at.trim().is_empty() {
        skill.updated_at = format_local_datetime();
    }

    Ok(skill)
}

/** 把用户输入的 name 归一化为适合 prompt、选择器和持久化使用的稳定标识。 */
fn normalize_skill_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/** 清理标签或语义线索，去重后限制数量，避免单个 skill 占用过多 prompt。 */
fn normalize_terms(terms: Vec<String>) -> Vec<String> {
    let mut seen_terms = HashSet::new();

    terms
        .into_iter()
        .map(|term| term.trim().to_owned())
        .filter(|term| !term.is_empty())
        .filter(|term| seen_terms.insert(term.to_lowercase()))
        .take(16)
        .collect()
}

/** 判断 ID 是否属于内置 skill，防止用户通过 payload 伪装覆盖内置定义。 */
fn is_built_in_skill_id(skill_id: &str) -> bool {
    built_in_skills().iter().any(|skill| skill.id == skill_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /** 创建只包含 agent_skills 表的内存数据库，避免单元测试依赖 Tauri AppHandle。 */
    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open in-memory sqlite");

        connection
            .execute_batch(
                r#"
                CREATE TABLE agent_skills (
                  id TEXT PRIMARY KEY,
                  source TEXT NOT NULL,
                  payload_json TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                "#,
            )
            .expect("create agent_skills table");

        connection
    }

    /** 写入一个文件式 skill 目录，并返回生成的 SKILL.md 路径。 */
    fn write_skill_markdown(root: &Path, folder: &str, markdown: &str) -> PathBuf {
        let skill_dir = root.join(folder);

        fs::create_dir_all(&skill_dir).expect("create skill directory");
        let skill_path = skill_dir.join(SKILL_MARKDOWN_FILE_NAME);

        fs::write(&skill_path, markdown).expect("write SKILL.md");

        skill_path
    }

    /** 写入首版支持的 agents/openai.yaml 元数据文件。 */
    fn write_openai_yaml(root: &Path, folder: &str, yaml: &str) {
        let metadata_dir = root.join(folder).join("agents");

        fs::create_dir_all(&metadata_dir).expect("create agents directory");
        fs::write(metadata_dir.join("openai.yaml"), yaml).expect("write openai.yaml");
    }

    /** 从合并列表中过滤文件式 skill，便于测试关注扫描结果。 */
    fn file_skills(skills: &[AgentSkill]) -> Vec<AgentSkill> {
        skills
            .iter()
            .filter(|skill| skill.source == FILE_SKILL_SOURCE)
            .cloned()
            .collect()
    }

    /** 创建一个最小有效 SKILL.md 文本，正文作为完整 instructions。 */
    fn valid_skill_markdown(name: &str, description: &str, instructions: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{instructions}\n")
    }

    /** 构造表单提交的用户 skill，测试保存逻辑会将它转换成文件式 skill。 */
    fn draft_user_skill(name: &str) -> AgentSkill {
        AgentSkill {
            id: String::new(),
            name: name.to_owned(),
            display_name: "测试 Skill".to_owned(),
            description: "用于验证文件式保存。".to_owned(),
            instructions: "执行测试 skill。".to_owned(),
            tags: vec!["测试".to_owned()],
            triggers: vec!["触发".to_owned()],
            enabled: true,
            source: USER_SKILL_SOURCE.to_owned(),
            allow_auto_invoke: true,
            created_at: String::new(),
            updated_at: String::new(),
            path: None,
            relative_path: None,
            metadata: None,
        }
    }

    /** 构造安装测试选项，默认模拟第三方来源且安装后停用。 */
    fn install_options(conflict_strategy: &str, enable_after_install: bool) -> SkillInstallOptions {
        SkillInstallOptions {
            source_type: "localFolder".to_owned(),
            source_summary: "local:test".to_owned(),
            enable_after_install,
            conflict_strategy: conflict_strategy.to_owned(),
        }
    }

    /** 创建测试 zip 字节，路径保持原样交给解包逻辑校验。 */
    fn build_zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();

        for (path, content) in entries {
            writer.start_file(path, options).expect("start zip file");
            writer
                .write_all(content.as_bytes())
                .expect("write zip content");
        }

        writer.finish().expect("finish zip").into_inner()
    }

    /** 未显式选择时不再根据 action 或触发词激活 Skill。 */
    #[test]
    fn does_not_activate_skill_without_explicit_selection() {
        let settings = crate::storage::default_user_settings();
        let request = AgentTurnRequest {
            prompt: "请处理当前笔记".to_owned(),
            action: "rewrite".to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            selected_skill_id: None,
        };

        let active_skill = resolve_active_skill(&built_in_skills(), &settings, &request);

        assert!(active_skill.is_none());
    }

    /** 显式选择必须注入完整 Skill，方便用户主动指定工作流。 */
    #[test]
    fn explicit_skill_selection_wins() {
        let settings = crate::storage::default_user_settings();
        let request = AgentTurnRequest {
            prompt: "请改写当前笔记".to_owned(),
            action: "rewrite".to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            selected_skill_id: Some("skill-note-research".to_owned()),
        };

        let active_skill = resolve_active_skill(&built_in_skills(), &settings, &request);

        assert_eq!(
            active_skill.map(|skill| skill.name),
            Some("note-research".to_owned())
        );
    }

    /** 用户目录中的 SKILL.md 应被扫描为 file 来源 skill，并保留稳定路径信息。 */
    #[test]
    fn scans_file_skill_from_user_root() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let skill_path = write_skill_markdown(
            &root,
            "demo",
            &valid_skill_markdown("demo-skill", "演示文件技能", "执行 demo 文件技能。"),
        );
        let connection = test_connection();
        let skills =
            load_agent_skills_from_roots(&connection, &[root.clone()]).expect("load skills");
        let files = file_skills(&skills);
        let skill = files.first().expect("file skill exists");

        assert_eq!(files.len(), 1);
        assert_eq!(skill.name, "demo-skill");
        assert_eq!(skill.display_name, "demo-skill");
        assert_eq!(skill.description, "演示文件技能");
        assert!(skill.instructions.contains("执行 demo 文件技能"));
        assert_eq!(skill.relative_path.as_deref(), Some("demo/SKILL.md"));
        assert_eq!(
            skill.path.as_deref(),
            Some(stable_absolute_path(&skill_path).to_string_lossy().as_ref())
        );
        assert!(skill.id.starts_with("skill-file-"));
    }

    /** 标准 YAML frontmatter 支持 display_name 和数组字段。 */
    #[test]
    fn parses_standard_yaml_frontmatter_fields() {
        let parsed = parse_skill_markdown(
            r#"---
name: yaml-demo
display_name: YAML 展示名
description: 标准 YAML 描述
tags:
  - 写作
  - 研究
triggers: ["总结", "summary"]
---

执行 YAML skill。
"#,
        )
        .expect("parse yaml skill");

        assert_eq!(parsed.name, "yaml-demo");
        assert_eq!(parsed.display_name.as_deref(), Some("YAML 展示名"));
        assert_eq!(parsed.tags, vec!["写作", "研究"]);
        assert_eq!(parsed.triggers, vec!["总结", "summary"]);
    }

    /** 安装本地多 skill 包时，应复制到用户目录并默认停用。 */
    #[test]
    fn installs_multiple_local_skills_disabled_by_default() {
        let temp_dir = tempdir().expect("create tempdir");
        let source = temp_dir.path().join("source");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();

        write_skill_markdown(
            &source,
            "one",
            &valid_skill_markdown("install-one", "安装技能一", "执行安装技能一。"),
        );
        write_skill_markdown(
            &source,
            "nested/two",
            &valid_skill_markdown("install-two", "安装技能二", "执行安装技能二。"),
        );

        let result = install_agent_skills_from_prepared_root(
            &connection,
            &root,
            &source,
            install_options(INSTALL_CONFLICT_FAIL, false),
        )
        .expect("install skills");

        assert_eq!(result.installed_count, 2);
        assert!(root
            .join("install-one")
            .join(SKILL_MARKDOWN_FILE_NAME)
            .exists());
        assert!(root
            .join("install-two")
            .join(SKILL_MARKDOWN_FILE_NAME)
            .exists());
        assert!(result.installed_skills.iter().all(|skill| !skill.enabled));
        assert!(result
            .installed_skills
            .iter()
            .all(|skill| !skill.allow_auto_invoke));
    }

    /** 安装包中的 scripts 目录会保留但给出不会执行的提示。 */
    #[test]
    fn install_keeps_scripts_but_returns_warning() {
        let temp_dir = tempdir().expect("create tempdir");
        let source = temp_dir.path().join("source");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();

        write_skill_markdown(
            &source,
            "scripted",
            &valid_skill_markdown("scripted", "带脚本技能", "执行前先阅读说明。"),
        );
        fs::create_dir_all(source.join("scripted/scripts")).expect("create scripts");
        fs::write(source.join("scripted/scripts/run.sh"), "echo test").expect("write script");

        let result = install_agent_skills_from_prepared_root(
            &connection,
            &root,
            &source,
            install_options(INSTALL_CONFLICT_FAIL, false),
        )
        .expect("install scripted skill");

        assert!(root.join("scripted/scripts/run.sh").exists());
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("不会执行脚本")));
    }

    /** 同名 skill 默认拒绝覆盖；replace 策略才会替换已有目录。 */
    #[test]
    fn install_conflict_fail_and_replace_behaviors() {
        let temp_dir = tempdir().expect("create tempdir");
        let source = temp_dir.path().join("source");
        let replacement = temp_dir.path().join("replacement");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();

        write_skill_markdown(
            &source,
            "conflict",
            &valid_skill_markdown("conflict-skill", "旧描述", "旧 instructions。"),
        );
        install_agent_skills_from_prepared_root(
            &connection,
            &root,
            &source,
            install_options(INSTALL_CONFLICT_FAIL, false),
        )
        .expect("install original");

        write_skill_markdown(
            &replacement,
            "conflict",
            &valid_skill_markdown("conflict-skill", "新描述", "新 instructions。"),
        );
        let failed = install_agent_skills_from_prepared_root(
            &connection,
            &root,
            &replacement,
            install_options(INSTALL_CONFLICT_FAIL, false),
        )
        .expect_err("conflict should fail");

        assert!(failed.contains("已存在"));

        install_agent_skills_from_prepared_root(
            &connection,
            &root,
            &replacement,
            install_options(INSTALL_CONFLICT_REPLACE, false),
        )
        .expect("replace skill");

        let markdown =
            fs::read_to_string(root.join("conflict-skill/SKILL.md")).expect("read replaced skill");

        assert!(markdown.contains("新 instructions"));
    }

    /** zip 包会安全解压并进入同一安装管线。 */
    #[test]
    fn installs_skill_from_zip_bytes() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();
        let bytes = build_zip_bytes(&[(
            "repo-main/skills/zip-demo/SKILL.md",
            &valid_skill_markdown("zip-demo", "zip 安装技能", "执行 zip 技能。"),
        )]);
        let prepared = prepare_skill_archive_bytes(&bytes).expect("prepare zip");
        let result = install_agent_skills_from_prepared_root(
            &connection,
            &root,
            prepared.path(),
            install_options(INSTALL_CONFLICT_FAIL, false),
        )
        .expect("install from zip");

        assert_eq!(result.installed_count, 1);
        assert!(root
            .join("zip-demo")
            .join(SKILL_MARKDOWN_FILE_NAME)
            .exists());
    }

    /** zip 中的路径穿越条目必须在解包阶段被拒绝。 */
    #[test]
    fn rejects_zip_path_traversal() {
        let bytes = build_zip_bytes(&[("../evil/SKILL.md", "bad")]);
        let error = prepare_skill_archive_bytes(&bytes).expect_err("path traversal rejected");

        assert!(error.contains("不安全路径"));
    }

    /** URL 解析应把 GitHub blob 和 tree 转换为下载地址，并保留脱敏摘要。 */
    #[test]
    fn resolves_github_skill_urls() {
        let blob = resolve_skill_url_download(
            "https://github.com/example/skills/blob/main/writing/SKILL.md",
        )
        .expect("resolve blob");
        let tree = resolve_skill_url_download("https://github.com/example/skills/tree/dev/writing")
            .expect("resolve tree");
        let clone_url = resolve_skill_url_download("https://github.com/obra/superpowers.git")
            .expect("resolve clone url");

        assert_eq!(blob.kind, SkillUrlDownloadKind::Markdown);
        assert_eq!(
            blob.url,
            "https://raw.githubusercontent.com/example/skills/main/writing/SKILL.md"
        );
        assert_eq!(tree.kind, SkillUrlDownloadKind::Archive);
        assert_eq!(
            tree.url,
            "https://github.com/example/skills/archive/refs/heads/dev.zip"
        );
        assert_eq!(blob.source_summary, "github.com/example/skills");
        assert_eq!(clone_url.kind, SkillUrlDownloadKind::Archive);
        assert_eq!(
            clone_url.url,
            "https://github.com/obra/superpowers/archive/refs/heads/main.zip"
        );
        assert_eq!(clone_url.source_summary, "github.com/obra/superpowers");
    }

    /** 单个无效 SKILL.md 只会被跳过，不应阻塞同根目录下其他有效文件式 skill。 */
    #[test]
    fn skips_invalid_file_skill_without_blocking_valid_skills() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");

        write_skill_markdown(
            &root,
            "valid",
            &valid_skill_markdown("valid-skill", "有效技能", "执行有效技能。"),
        );
        write_skill_markdown(
            &root,
            "missing-description",
            "---\nname: broken-skill\n---\n\n正文存在但缺少 description。\n",
        );
        let connection = test_connection();
        let skills = load_agent_skills_from_roots(&connection, &[root]).expect("load skills");
        let files = file_skills(&skills);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "valid-skill");
    }

    /** agents/openai.yaml 可以覆盖展示名称，并关闭隐式模型参考。 */
    #[test]
    fn openai_yaml_can_disable_file_skill_auto_invocation() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");

        write_skill_markdown(
            &root,
            "demo",
            &valid_skill_markdown("demo-skill", "演示文件技能", "执行 demo 文件技能。"),
        );
        write_openai_yaml(
            &root,
            "demo",
            "interface:\n  display_name: \"演示 Skill\"\npolicy:\n  allow_implicit_invocation: false\n",
        );
        let connection = test_connection();
        let skills = load_agent_skills_from_roots(&connection, &[root]).expect("load skills");
        let skill = file_skills(&skills).pop().expect("file skill exists");

        assert_eq!(skill.display_name, "演示 Skill");
        assert!(!skill.allow_auto_invoke);
        assert_eq!(
            skill
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("allowAutoInvokeSource"))
                .map(String::as_str),
            Some("agents/openai.yaml")
        );
    }

    /** UI 新建的 skill 应写成 SKILL.md 文件，并只在 SQLite 中保存状态覆盖。 */
    #[test]
    fn save_user_skill_writes_skill_markdown_to_user_root() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();
        let mut draft = draft_user_skill("custom-save");

        draft.allow_auto_invoke = false;
        let saved = save_user_skill_to_root(&connection, &root, draft).expect("save user skill");
        let skill_path = root.join("custom-save").join(SKILL_MARKDOWN_FILE_NAME);
        let metadata_path = root.join("custom-save").join("agents").join("openai.yaml");
        let markdown = fs::read_to_string(&skill_path).expect("read written SKILL.md");
        let metadata = fs::read_to_string(metadata_path).expect("read written openai.yaml");
        let persisted = read_persisted_skills(&connection).expect("read persisted override");
        let override_skill = persisted.get(&saved.id).expect("override exists");

        assert_eq!(saved.source, FILE_SKILL_SOURCE);
        assert_eq!(saved.relative_path.as_deref(), Some("custom-save/SKILL.md"));
        assert!(markdown.contains("name: \"custom-save\""));
        assert!(markdown.contains("description: \"用于验证文件式保存。\""));
        assert!(markdown.contains("执行测试 skill。"));
        assert!(metadata.contains("display_name: \"测试 Skill\""));
        assert!(metadata.contains("allow_implicit_invocation: false"));
        assert_eq!(override_skill.source, FILE_SKILL_SOURCE);
        assert!(override_skill.instructions.is_empty());
        assert!(!override_skill.allow_auto_invoke);
    }

    /** 编辑已有文件式 skill 改动 name 时，应同步迁移目录并清理旧路径状态覆盖。 */
    #[test]
    fn editing_file_skill_renames_directory_when_name_changes() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();
        let saved =
            save_user_skill_to_root(&connection, &root, draft_user_skill("original-folder"))
                .expect("save first skill");
        let mut edited = saved.clone();

        edited.name = "renamed-skill".to_owned();
        edited.display_name = "改名后的 Skill".to_owned();
        edited.instructions = "改名后写回新目录。".to_owned();
        let resaved =
            save_user_skill_to_root(&connection, &root, edited).expect("save edited skill");
        let original_path = root.join("original-folder").join(SKILL_MARKDOWN_FILE_NAME);
        let renamed_path = root.join("renamed-skill").join(SKILL_MARKDOWN_FILE_NAME);
        let markdown = fs::read_to_string(renamed_path).expect("read renamed SKILL.md");
        let persisted = read_persisted_skills(&connection).expect("read persisted override");

        assert_ne!(resaved.id, saved.id);
        assert_eq!(
            resaved.relative_path.as_deref(),
            Some("renamed-skill/SKILL.md")
        );
        assert!(markdown.contains("name: \"renamed-skill\""));
        assert!(markdown.contains("改名后写回新目录。"));
        assert!(!original_path.exists());
        assert!(!persisted.contains_key(&saved.id));
        assert!(persisted.contains_key(&resaved.id));
    }

    /** 删除文件式 skill 应删除整个用户 skill 目录，并清理 SQLite 状态覆盖。 */
    #[test]
    fn delete_user_file_skill_removes_directory_and_override() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();
        let saved = save_user_skill_to_root(&connection, &root, draft_user_skill("delete-me"))
            .expect("save skill");

        assert!(root
            .join("delete-me")
            .join(SKILL_MARKDOWN_FILE_NAME)
            .exists());

        delete_user_skill_from_root(&connection, &root, &saved.id).expect("delete file skill");
        let persisted = read_persisted_skills(&connection).expect("read persisted after delete");

        assert!(!root.join("delete-me").exists());
        assert!(!persisted.contains_key(&saved.id));
    }

    /** 文件 skill 的启停和模型参考偏好应按路径 ID 保存在 SQLite 中并在重新加载时生效。 */
    #[test]
    fn file_skill_toggle_override_persists_across_loads() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");

        write_skill_markdown(
            &root,
            "demo",
            &valid_skill_markdown("demo-skill", "演示文件技能", "执行 demo 文件技能。"),
        );
        let connection = test_connection();
        let mut skill = file_skills(
            &load_agent_skills_from_roots(&connection, &[root.clone()]).expect("load skills"),
        )
        .pop()
        .expect("file skill exists");

        skill.enabled = false;
        skill.allow_auto_invoke = false;
        skill.updated_at = "覆盖时间".to_owned();
        upsert_skill_state_override(&connection, &skill).expect("persist override");

        let reloaded = file_skills(
            &load_agent_skills_from_roots(&connection, &[root]).expect("reload skills"),
        )
        .pop()
        .expect("file skill still exists");

        assert_eq!(reloaded.id, skill.id);
        assert!(!reloaded.enabled);
        assert!(!reloaded.allow_auto_invoke);
        assert_eq!(reloaded.updated_at, "覆盖时间");
    }

    /** 历史 SQLite user skill 会在加载时迁移到文件目录，并保留禁用状态。 */
    #[test]
    fn legacy_user_skill_migrates_to_file_skill_on_load() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let connection = test_connection();
        let mut legacy = draft_user_skill("legacy-skill");

        legacy.id = "legacy-user-skill".to_owned();
        legacy.enabled = false;
        legacy.allow_auto_invoke = false;
        legacy.updated_at = "旧时间".to_owned();
        upsert_skill(&connection, &legacy).expect("insert legacy user skill");

        let loaded =
            load_agent_skills_from_roots(&connection, &[root.clone()]).expect("load skills");
        let migrated = file_skills(&loaded)
            .into_iter()
            .find(|skill| skill.name == "legacy-skill")
            .expect("migrated file skill");
        let persisted = read_persisted_skills(&connection).expect("read persisted after migration");

        assert!(root
            .join("legacy-skill")
            .join(SKILL_MARKDOWN_FILE_NAME)
            .exists());
        assert!(!migrated.enabled);
        assert!(!migrated.allow_auto_invoke);
        assert!(!persisted.contains_key("legacy-user-skill"));
        assert!(persisted.contains_key(&migrated.id));
    }

    /** 文件式 skill 的正文每次从磁盘读取，修改 SKILL.md 后下一次加载应返回新 instructions。 */
    #[test]
    fn file_skill_instructions_refresh_when_markdown_changes() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let skill_path = write_skill_markdown(
            &root,
            "demo",
            &valid_skill_markdown("demo-skill", "演示文件技能", "第一版 instructions。"),
        );
        let connection = test_connection();
        let first = file_skills(
            &load_agent_skills_from_roots(&connection, &[root.clone()]).expect("load first"),
        )
        .pop()
        .expect("first file skill");

        fs::write(
            &skill_path,
            valid_skill_markdown("demo-skill", "演示文件技能", "第二版 instructions。"),
        )
        .expect("rewrite SKILL.md");

        let second =
            file_skills(&load_agent_skills_from_roots(&connection, &[root]).expect("load second"))
                .pop()
                .expect("second file skill");

        assert_eq!(second.id, first.id);
        assert!(second.instructions.contains("第二版"));
        assert!(!second.instructions.contains("第一版"));
    }

    /** 删除磁盘上的 SKILL.md 后，下一次加载不应继续展示旧的文件式 skill。 */
    #[test]
    fn removed_file_skill_disappears_from_loaded_catalog() {
        let temp_dir = tempdir().expect("create tempdir");
        let root = temp_dir.path().join("skills");
        let skill_path = write_skill_markdown(
            &root,
            "demo",
            &valid_skill_markdown("demo-skill", "演示文件技能", "执行 demo 文件技能。"),
        );
        let connection = test_connection();

        assert_eq!(
            file_skills(
                &load_agent_skills_from_roots(&connection, &[root.clone()]).expect("load skills")
            )
            .len(),
            1
        );

        fs::remove_file(skill_path).expect("remove SKILL.md");

        assert!(file_skills(
            &load_agent_skills_from_roots(&connection, &[root]).expect("reload skills")
        )
        .is_empty());
    }
}
