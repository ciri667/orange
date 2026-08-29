use super::catalog::{
    normalize_skill_name, should_skip_install_relative_path, SKILL_MARKDOWN_FILE_NAME,
};
use crate::domain::OnlineSkill;
use serde::Deserialize;
use std::path::Path;

/** skills.sh 搜索接口根地址，可用环境变量覆盖以便测试。 */
pub const SKILLS_SH_API_BASE: &str = "https://skills.sh";

/** 搜索关键词最短长度，与 skills CLI 的交互搜索一致。 */
pub const MIN_ONLINE_SKILL_QUERY_CHARS: usize = 2;

/** 搜索关键词最长长度，避免把异常长输入发给目录服务。 */
const MAX_ONLINE_SKILL_QUERY_CHARS: usize = 80;

/** 单次搜索最多返回条数。 */
pub const ONLINE_SKILL_SEARCH_LIMIT: u32 = 20;

/** 预览简介最大字符数，避免把整页 HTML 元数据塞进 UI。 */
const MAX_ONLINE_SKILL_DESCRIPTION_CHARS: usize = 500;

/** GitHub owner 允许的最大长度。 */
const MAX_GITHUB_OWNER_CHARS: usize = 39;

/** skills.sh 搜索接口的原始条目。 */
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsShSearchSkill {
    #[serde(default)]
    id: String,
    #[serde(default)]
    skill_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    installs: u64,
}

/** skills.sh 搜索接口的原始响应。 */
#[derive(Clone, Debug, Deserialize)]
struct SkillsShSearchResponse {
    #[serde(default)]
    skills: Vec<SkillsShSearchSkill>,
}

/** GitHub git tree 接口中的单个条目。 */
#[derive(Clone, Debug, Deserialize)]
pub struct GitHubTreeEntry {
    #[serde(default)]
    pub path: String,
    #[serde(rename = "type", default)]
    pub entry_type: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/** GitHub recursive tree 接口响应。 */
#[derive(Clone, Debug, Deserialize)]
pub struct GitHubTreeResponse {
    #[serde(default)]
    pub tree: Vec<GitHubTreeEntry>,
    #[serde(default)]
    pub truncated: bool,
}

/** GitHub 仓库元数据，只读取默认分支。 */
#[derive(Clone, Debug, Deserialize)]
pub struct GitHubRepoResponse {
    #[serde(default)]
    pub default_branch: String,
}

/** 已解析的 GitHub skill 目录，files 只包含该目录下的 blob。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubSkillDirectory {
    pub prefix: String,
    pub files: Vec<GitHubTreeFile>,
}

/** 待下载的 GitHub 文件，path 相对仓库根目录。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubTreeFile {
    pub path: String,
    pub size: Option<u64>,
}

/** 在线搜索查询校验失败原因。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnlineSkillQueryError {
    TooShort,
    TooLong,
    InvalidOwner,
}

/** 从 GitHub tree 解析 skill 目录失败时，调用方应回退到 zip 安装。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHubSkillResolveError {
    Truncated,
    NotFound,
    InvalidName,
}

/** 校验搜索词和可选 owner，通过后返回已修剪值。 */
pub fn normalize_online_skill_query(
    query: &str,
    owner: Option<&str>,
) -> Result<(String, Option<String>), OnlineSkillQueryError> {
    let normalized_query = query.trim();

    if normalized_query.chars().count() < MIN_ONLINE_SKILL_QUERY_CHARS {
        return Err(OnlineSkillQueryError::TooShort);
    }

    if normalized_query.chars().count() > MAX_ONLINE_SKILL_QUERY_CHARS {
        return Err(OnlineSkillQueryError::TooLong);
    }

    let normalized_owner = owner
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    if let Some(owner) = normalized_owner.as_deref() {
        if !is_github_owner(owner) {
            return Err(OnlineSkillQueryError::InvalidOwner);
        }
    }

    Ok((normalized_query.to_owned(), normalized_owner))
}

/** 把查询错误转成设置页可展示的中文说明。 */
pub fn online_skill_query_error_message(error: OnlineSkillQueryError) -> String {
    match error {
        OnlineSkillQueryError::TooShort => "请输入至少两个字搜索在线 Skills。".to_owned(),
        OnlineSkillQueryError::TooLong => "搜索词过长，请缩短后再试。".to_owned(),
        OnlineSkillQueryError::InvalidOwner => "来源 owner 无效。".to_owned(),
    }
}

/** 解析 skills.sh 搜索 JSON，丢掉空 id，并补上可打开的详情页地址。 */
pub fn parse_online_skill_search_response(
    query: &str,
    body: &str,
) -> Result<Vec<OnlineSkill>, String> {
    let parsed: SkillsShSearchResponse = serde_json::from_str(body)
        .map_err(|_| "在线 Skill 目录返回了无法解析的结果。".to_owned())?;

    let mut skills = parsed
        .skills
        .into_iter()
        .filter_map(|skill| {
            let id = skill.id.trim().to_owned();
            let page_url = online_skill_page_url(&id)?;
            let skill_id = if skill.skill_id.trim().is_empty() {
                skill.name.trim().to_owned()
            } else {
                skill.skill_id.trim().to_owned()
            };
            let name = if skill.name.trim().is_empty() {
                skill_id.clone()
            } else {
                skill.name.trim().to_owned()
            };

            if skill_id.is_empty() || name.is_empty() {
                return None;
            }

            let source = skill.source.trim().to_owned();

            Some(OnlineSkill {
                id,
                skill_id,
                name,
                installable: parse_github_owner_repo(&source).is_some(),
                source,
                installs: skill.installs,
                page_url,
                description: None,
            })
        })
        .take(ONLINE_SKILL_SEARCH_LIMIT as usize)
        .collect::<Vec<_>>();

    skills.sort_by(|left, right| right.installs.cmp(&left.installs));
    let _ = query;

    Ok(skills)
}

/** 构造 skills.sh 详情页地址；拒绝路径穿越和查询串。 */
pub fn online_skill_page_url(id: &str) -> Option<String> {
    let normalized_id = id.trim().trim_start_matches('/');

    if normalized_id.is_empty()
        || normalized_id.contains("..")
        || normalized_id.contains('?')
        || normalized_id.contains('#')
        || normalized_id.contains('\\')
    {
        return None;
    }

    if !normalized_id.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
    }) {
        return None;
    }

    Some(format!("{SKILLS_SH_API_BASE}/{normalized_id}"))
}

/** 从 HTML 中提取 og:description 或 description，失败时返回 None。 */
pub fn extract_html_description(html: &str) -> Option<String> {
    extract_meta_content(html, "og:description")
        .or_else(|| extract_meta_content(html, "description"))
        .and_then(|value| {
            let trimmed = decode_html_entities(&value);
            let trimmed = trimmed.trim();

            if trimmed.is_empty() {
                None
            } else {
                Some(truncate_chars(trimmed, MAX_ONLINE_SKILL_DESCRIPTION_CHARS))
            }
        })
}

/** 解析 GitHub owner/repo 简写，拒绝多余路径和空段。 */
pub fn parse_github_owner_repo(source: &str) -> Option<(String, String)> {
    let trimmed = source.trim().trim_end_matches(".git");
    let mut parts = trimmed.split('/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();

    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }

    if !is_github_owner(owner) || !is_github_repo_name(repo) {
        return None;
    }

    Some((owner.to_owned(), repo.to_owned()))
}

/** 发现页安装目标：仅在 URL 指向仓库根且指定了单个 skill 时走目录下载。 */
pub fn github_named_skill_install_target(
    url: &str,
    skill_names: &[String],
) -> Option<(String, String, String)> {
    if skill_names.len() != 1 {
        return None;
    }

    let skill_name = normalize_skill_name(&skill_names[0]);

    if skill_name.is_empty() {
        return None;
    }

    let parsed_url = reqwest::Url::parse(url.trim()).ok()?;

    if parsed_url.scheme() != "https" || parsed_url.host_str() != Some("github.com") {
        return None;
    }

    let parts = parsed_url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if parts.len() != 2 {
        return None;
    }

    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");

    if !is_github_owner(owner) || !is_github_repo_name(repo) {
        return None;
    }

    Some((owner.to_owned(), repo.to_owned(), skill_name))
}

/** 在 GitHub tree 中定位与 skill 名称匹配的目录。 */
pub fn find_github_skill_directory(
    tree: &GitHubTreeResponse,
    skill_name: &str,
) -> Result<GitHubSkillDirectory, GitHubSkillResolveError> {
    if tree.truncated {
        return Err(GitHubSkillResolveError::Truncated);
    }

    let wanted = normalize_skill_name(skill_name);

    if wanted.is_empty() {
        return Err(GitHubSkillResolveError::InvalidName);
    }

    let mut candidates = Vec::new();

    for entry in &tree.tree {
        if entry.entry_type != "blob" {
            continue;
        }

        let path = normalize_github_path(&entry.path);

        if path.is_empty() || should_skip_github_path(&path) {
            continue;
        }

        if !is_skill_markdown_path(&path) {
            continue;
        }

        let directory = skill_directory_prefix(&path);
        let folder_name = directory
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(directory);

        if normalize_skill_name(folder_name) == wanted {
            candidates.push(directory.to_owned());
        }
    }

    candidates.sort_by_key(|directory| (directory.matches('/').count(), directory.len()));
    candidates.dedup();

    let prefix = candidates
        .into_iter()
        .next()
        .ok_or(GitHubSkillResolveError::NotFound)?;

    let files = tree
        .tree
        .iter()
        .filter(|entry| entry.entry_type == "blob")
        .filter_map(|entry| {
            let path = normalize_github_path(&entry.path);

            if path.is_empty()
                || should_skip_github_path(&path)
                || !path_is_under_prefix(&path, &prefix)
            {
                return None;
            }

            Some(GitHubTreeFile {
                path,
                size: entry.size,
            })
        })
        .collect::<Vec<_>>();

    if !files.iter().any(|file| is_skill_markdown_path(&file.path)) {
        return Err(GitHubSkillResolveError::NotFound);
    }

    Ok(GitHubSkillDirectory { prefix, files })
}

/** 把 GitHub 目录前缀转成安装临时目录中的相对根，使 SKILL.md 落在 skill 文件夹内。 */
pub fn github_file_relative_path(prefix: &str, path: &str) -> Option<String> {
    let path = normalize_github_path(path);

    if prefix.is_empty() {
        return Some(path);
    }

    if path == prefix {
        return None;
    }

    let prefix_with_slash = format!("{prefix}/");
    path.strip_prefix(&prefix_with_slash)
        .map(ToOwned::to_owned)
        .filter(|relative| !relative.is_empty())
}

/** 判断相对路径是否应在 GitHub 目录下载时跳过。 */
pub fn should_skip_github_path(path: &str) -> bool {
    should_skip_install_relative_path(Path::new(path))
}

fn is_github_owner(owner: &str) -> bool {
    let owner = owner.trim();
    let char_count = owner.chars().count();

    if char_count == 0 || char_count > MAX_GITHUB_OWNER_CHARS {
        return false;
    }

    let mut characters = owner.chars();
    let Some(first) = characters.next() else {
        return false;
    };

    first.is_ascii_alphanumeric()
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn is_github_repo_name(repo: &str) -> bool {
    let repo = repo.trim();

    !repo.is_empty()
        && repo.len() <= 100
        && repo.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn normalize_github_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_owned()
}

fn is_skill_markdown_path(path: &str) -> bool {
    path == SKILL_MARKDOWN_FILE_NAME || path.ends_with(&format!("/{SKILL_MARKDOWN_FILE_NAME}"))
}

fn skill_directory_prefix(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("")
}

fn path_is_under_prefix(path: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }

    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn extract_meta_content(html: &str, key: &str) -> Option<String> {
    let mut search_from = 0;

    while let Some(offset) = html[search_from..].find(key) {
        let absolute_index = search_from + offset;
        let Some(tag_start) = html[..absolute_index].rfind("<meta") else {
            search_from = absolute_index + key.len();
            continue;
        };
        let Some(relative_end) = html[absolute_index..].find('>') else {
            search_from = absolute_index + key.len();
            continue;
        };
        let tag_end = absolute_index + relative_end;

        if tag_end > tag_start {
            let tag = &html[tag_start..tag_end];

            if let Some(content) = meta_tag_attr(tag, "content") {
                if !content.trim().is_empty() {
                    return Some(content);
                }
            }
        }

        search_from = absolute_index + key.len();
    }

    None
}

fn meta_tag_attr(tag: &str, attr: &str) -> Option<String> {
    let pattern_double = format!("{attr}=\"");
    let pattern_single = format!("{attr}='");

    if let Some(start) = tag.find(&pattern_double) {
        let value_start = start + pattern_double.len();
        let value_end = tag[value_start..].find('"')? + value_start;
        return Some(tag[value_start..value_end].to_owned());
    }

    if let Some(start) = tag.find(&pattern_single) {
        let value_start = start + pattern_single.len();
        let value_end = tag[value_start..].find('\'')? + value_start;
        return Some(tag[value_start..value_end].to_owned());
    }

    None
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut truncated = value.chars().take(max_chars).collect::<String>();

    if value.chars().count() > max_chars {
        truncated.push('…');
    }

    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    /** 搜索 JSON 应按安装量排序，并标记 GitHub 来源可安装。 */
    #[test]
    fn parses_skills_sh_search_response() {
        let body = r#"{
            "query": "writing",
            "skills": [
                {
                    "id": "open.feishu.cn/lark-note",
                    "skillId": "lark-note",
                    "name": "lark-note",
                    "installs": 10,
                    "source": "open.feishu.cn"
                },
                {
                    "id": "vercel-labs/agent-skills/writing-guidelines",
                    "skillId": "writing-guidelines",
                    "name": "writing-guidelines",
                    "installs": 54516,
                    "source": "vercel-labs/agent-skills"
                }
            ]
        }"#;
        let skills = parse_online_skill_search_response("writing", body).expect("parse search");

        assert_eq!(skills[0].skill_id, "writing-guidelines");
        assert!(skills[0].installable);
        assert_eq!(
            skills[0].page_url,
            "https://skills.sh/vercel-labs/agent-skills/writing-guidelines"
        );
        assert!(!skills[1].installable);
    }

    /** 过短查询和非法 owner 应在发请求前被拒绝。 */
    #[test]
    fn rejects_short_query_and_invalid_owner() {
        assert_eq!(
            normalize_online_skill_query("a", None),
            Err(OnlineSkillQueryError::TooShort)
        );
        assert_eq!(
            normalize_online_skill_query("pdf", Some("not a owner")),
            Err(OnlineSkillQueryError::InvalidOwner)
        );
        assert_eq!(
            normalize_online_skill_query("  pdf  ", Some("Anthropics")),
            Ok(("pdf".to_owned(), Some("anthropics".to_owned())))
        );
    }

    /** 详情页地址必须拒绝穿越和查询串。 */
    #[test]
    fn rejects_unsafe_skill_page_ids() {
        assert!(online_skill_page_url("../etc/passwd").is_none());
        assert!(online_skill_page_url("anthropics/skills/pdf?x=1").is_none());
        assert_eq!(
            online_skill_page_url("anthropics/skills/pdf").as_deref(),
            Some("https://skills.sh/anthropics/skills/pdf")
        );
    }

    /** 简介提取应读取 og:description 并解码实体。 */
    #[test]
    fn extracts_og_description() {
        let html = r#"<meta property="og:description" content="Use this skill for PDF files &amp; tables."/>"#;

        assert_eq!(
            extract_html_description(html).as_deref(),
            Some("Use this skill for PDF files & tables.")
        );
    }

    /** 仓库根 URL 加单个 skill 名称才走目录下载。 */
    #[test]
    fn resolves_named_github_install_target() {
        let target = github_named_skill_install_target(
            "https://github.com/anthropics/skills",
            &["pdf".to_owned()],
        )
        .expect("named target");

        assert_eq!(
            target,
            (
                "anthropics".to_owned(),
                "skills".to_owned(),
                "pdf".to_owned()
            )
        );
        assert!(github_named_skill_install_target(
            "https://github.com/anthropics/skills/tree/main/skills/pdf",
            &["pdf".to_owned()],
        )
        .is_none());
        assert!(
            github_named_skill_install_target("https://github.com/anthropics/skills", &[],)
                .is_none()
        );
    }

    /** tree 中同名目录应选择更浅的路径，并只收集该目录文件。 */
    #[test]
    fn finds_shallowest_matching_skill_directory() {
        let tree = GitHubTreeResponse {
            truncated: false,
            tree: vec![
                tree_blob("examples/pdf/SKILL.md", 12),
                tree_blob("skills/pdf/SKILL.md", 20),
                tree_blob("skills/pdf/scripts/extract.py", 40),
                tree_blob("skills/pdf/references/spec.md", 8),
                tree_blob("skills/docx/SKILL.md", 10),
                tree_blob("node_modules/pdf/SKILL.md", 1),
            ],
        };
        let directory = find_github_skill_directory(&tree, "pdf").expect("find pdf");

        assert_eq!(directory.prefix, "skills/pdf");
        assert_eq!(directory.files.len(), 3);
        assert!(directory
            .files
            .iter()
            .any(|file| file.path == "skills/pdf/scripts/extract.py"));
        assert_eq!(
            github_file_relative_path(&directory.prefix, "skills/pdf/SKILL.md").as_deref(),
            Some("SKILL.md")
        );
    }

    /** 截断的 tree 必须回退，避免漏装附属文件。 */
    #[test]
    fn truncated_tree_returns_fallback_error() {
        let tree = GitHubTreeResponse {
            truncated: true,
            tree: vec![tree_blob("skills/pdf/SKILL.md", 20)],
        };

        assert_eq!(
            find_github_skill_directory(&tree, "pdf"),
            Err(GitHubSkillResolveError::Truncated)
        );
    }

    fn tree_blob(path: &str, size: u64) -> GitHubTreeEntry {
        GitHubTreeEntry {
            path: path.to_owned(),
            entry_type: "blob".to_owned(),
            size: Some(size),
        }
    }
}
