use super::catalog::{
    hash_skill_directory, load_agent_skills_from_roots, load_custom_skill, normalize_skill_name,
    parse_skill_markdown, read_persisted_skills, safe_skill_folder_name,
    should_skip_install_relative_path, should_walk_skill_entry, upsert_skill_state_override,
    yaml_quote, ORANGE_INSTALL_METADATA_FILE_NAME, SKILL_MARKDOWN_FILE_NAME,
};
use crate::domain::InstallAgentSkillResult;
use crate::storage::{create_id, format_local_datetime};
use rusqlite::Connection;
use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::ZipArchive;

/** 第三方 skill 安装时允许复制的最大普通文件数量。 */
pub const MAX_SKILL_INSTALL_FILE_COUNT: usize = 512;

/** 第三方 skill 安装时允许复制的单文件最大字节数。 */
pub const MAX_SKILL_INSTALL_SINGLE_FILE_BYTES: u64 = 5 * 1024 * 1024;

/** 第三方 skill 安装时允许复制的总字节数。 */
pub const MAX_SKILL_INSTALL_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

/** 远程下载的单个 SKILL.md 最大字节数。 */
pub const MAX_REMOTE_SKILL_MARKDOWN_BYTES: usize = 1024 * 1024;

/** 远程下载的压缩包最大字节数；解压后还会再次做总量限制。 */
pub const MAX_REMOTE_SKILL_ARCHIVE_BYTES: usize = 25 * 1024 * 1024;

/** 第三方 skill 安装时保存在 agents 目录中的橘记元数据文件。 */
/** 第三方 skill 安装冲突时直接失败，不覆盖用户现有目录。 */
pub(super) const INSTALL_CONFLICT_FAIL: &str = "fail";

/** 第三方 skill 安装冲突时替换同名目录。 */
pub(super) const INSTALL_CONFLICT_REPLACE: &str = "replace";

/** 安装来源已经准备成目录后，将其中的标准 SKILL.md 包复制到用户 skills 根目录。 */
pub fn install_agent_skills_from_prepared_root(
    connection: &Connection,
    skills_root: &Path,
    prepared_root: &Path,
    options: SkillInstallOptions,
) -> Result<InstallAgentSkillResult, String> {
    let operation_started_at = format_local_datetime();
    let discovered_skills = filter_discovered_skills(
        discover_installable_skills(prepared_root)?,
        &options.skill_names,
    )?;

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
        .map(|skill_path| load_custom_skill(skills_root, skill_path, &mut persisted_skills))
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
    /** 非空时只安装名称匹配的 Skill；空列表表示安装来源中的全部 Skill。 */
    pub skill_names: Vec<String>,
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

/** 按用户指定的 skill 名称过滤安装项，避免发现页把整个仓库装进来。 */
fn filter_discovered_skills(
    skills: Vec<DiscoveredInstallableSkill>,
    skill_names: &[String],
) -> Result<Vec<DiscoveredInstallableSkill>, String> {
    if skill_names.is_empty() {
        return Ok(skills);
    }

    let wanted_names = skill_names
        .iter()
        .map(|name| normalize_skill_name(name))
        .filter(|name| !name.is_empty())
        .collect::<HashSet<_>>();

    if wanted_names.is_empty() {
        return Err("指定的 Skill 名称无效。".to_owned());
    }

    let filtered = skills
        .into_iter()
        .filter(|skill| skill_matches_requested_name(skill, &wanted_names))
        .collect::<Vec<_>>();
    let found_names = filtered
        .iter()
        .flat_map(discovered_skill_match_names)
        .collect::<HashSet<_>>();
    let missing_names = skill_names
        .iter()
        .filter(|name| {
            let normalized = normalize_skill_name(name);
            !normalized.is_empty() && !found_names.contains(&normalized)
        })
        .cloned()
        .collect::<Vec<_>>();

    if !missing_names.is_empty() {
        return Err(format!(
            "安装来源中没有找到 Skill「{}」。",
            missing_names.join("、")
        ));
    }

    Ok(filtered)
}

/** 返回可用于匹配安装过滤的名称：frontmatter name 和来源目录名。 */
fn discovered_skill_match_names(skill: &DiscoveredInstallableSkill) -> Vec<String> {
    let mut names = vec![normalize_skill_name(&skill.target_folder_name)];

    if let Some(folder_name) = skill
        .source_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(normalize_skill_name)
        .filter(|name| !name.is_empty())
    {
        names.push(folder_name);
    }

    names
}

/** 用 frontmatter name 或来源目录名匹配用户指定的 skill。 */
fn skill_matches_requested_name(
    skill: &DiscoveredInstallableSkill,
    wanted_names: &HashSet<String>,
) -> bool {
    if wanted_names.contains(&normalize_skill_name(&skill.target_folder_name)) {
        return true;
    }

    skill
        .source_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| wanted_names.contains(&normalize_skill_name(name)))
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

    write_orange_install_metadata(
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
        load_custom_skill(skills_root, &skill_markdown_path, &mut persisted_skills)?;

    installed_skill.enabled = options.enable_after_install;
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
                warnings.push("安装包包含 scripts 目录；仅声明 agents/orange-runtime.yaml 且通过权限审批后才可执行。".to_owned());
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

/** 写入橘记安装元数据，保留可审计摘要但不保存完整来源 URL 或绝对路径。 */
fn write_orange_install_metadata(
    skill_dir: &Path,
    discovered_skill: &DiscoveredInstallableSkill,
    options: &SkillInstallOptions,
    installed_at: &str,
    file_count: usize,
) -> Result<(), String> {
    let agents_dir = skill_dir.join("agents");
    let metadata_path = agents_dir.join(ORANGE_INSTALL_METADATA_FILE_NAME);
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
