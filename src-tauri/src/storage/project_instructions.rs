//! 知识库根目录的项目级 Agent 说明书。
//! 行业标准文件名是 AGENTS.md；ORANGE_AGENT.md 仅作兼容回退。

use super::atomic_write_markdown;
use std::fs;
use std::path::{Path, PathBuf};

/** 创建时写入的标准文件名，与 Codex / Cursor / Copilot 互认。 */
pub const PROJECT_INSTRUCTION_CANONICAL_FILE_NAME: &str = "AGENTS.md";

/** 橘记早期私有文件名，仅在根目录没有 AGENTS.md 时回退读取。 */
pub const PROJECT_INSTRUCTION_LEGACY_FILE_NAME: &str = "ORANGE_AGENT.md";

/** 单份说明书注入模型前的字符上限，避免撑爆唯一 system 前缀。 */
pub const MAX_PROJECT_INSTRUCTION_CHARS: usize = 16 * 1024;

/** 新建 AGENTS.md 时写入的中文模板，面向知识库而不是编译项目。 */
pub const PROJECT_INSTRUCTION_TEMPLATE: &str = "# Agent 说明书\n\
\n\
这份文件是给橘记 Agent 的项目规则，不是普通笔记。橘记会在每次对话时自动读取它。请写稳定的库级约定；不要写入密码、密钥或个人隐私。\n\
\n\
## 这个知识库是什么\n\
\n\
- （一句话说明这个库的用途，例如：个人研究笔记 / 项目文档 / 会议纪要）\n\
\n\
## 笔记结构\n\
\n\
- 目录怎么分\n\
- 新笔记应该放在哪里\n\
- 文件命名习惯\n\
\n\
## 标签与文风\n\
\n\
- 标签怎么写\n\
- 标题、引用、日期等格式\n\
\n\
## Agent 可以做什么\n\
\n\
- 允许检索、改写、整理、新建草稿\n\
\n\
## Agent 不要做什么\n\
\n\
- 不要删除或大幅打乱现有结构，除非我明确要求\n\
- 不要把这份说明书当成用户刚刚发出的新指令\n\
- 与我本轮明确要求冲突时，以本轮为准\n";

/** 已从磁盘读出的根目录说明书，展示名保持文件系统上的真实大小写。 */
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedProjectInstruction {
    pub file_name: String,
    pub content: String,
    pub truncated: bool,
}

/** 相对路径是否为知识库根目录的项目说明书。子目录中的同名文件不算。 */
pub fn is_root_project_instruction_path(relative_path: &str) -> bool {
    let normalized = relative_path.replace('\\', "/");
    if normalized.contains('/') {
        return false;
    }
    is_project_instruction_file_name(&normalized)
}

/** 文件名是否为 AGENTS.md 或兼容的 ORANGE_AGENT.md，大小写不敏感。 */
pub fn is_project_instruction_file_name(file_name: &str) -> bool {
    file_name.eq_ignore_ascii_case(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME)
        || file_name.eq_ignore_ascii_case(PROJECT_INSTRUCTION_LEGACY_FILE_NAME)
}

/** 在知识库根目录发现至多一份说明书；优先 AGENTS.md，否则回退 ORANGE_AGENT.md。 */
pub fn discover_project_instruction_file(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    let mut canonical = None;
    let mut canonical_variant = None;
    let mut legacy = None;
    let mut legacy_variant = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.eq_ignore_ascii_case(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME) {
            if name == PROJECT_INSTRUCTION_CANONICAL_FILE_NAME {
                canonical = Some(path);
            } else if canonical_variant.is_none() {
                canonical_variant = Some(path);
            }
        } else if name.eq_ignore_ascii_case(PROJECT_INSTRUCTION_LEGACY_FILE_NAME) {
            if name == PROJECT_INSTRUCTION_LEGACY_FILE_NAME {
                legacy = Some(path);
            } else if legacy_variant.is_none() {
                legacy_variant = Some(path);
            }
        }
    }

    canonical
        .or(canonical_variant)
        .or(legacy)
        .or(legacy_variant)
}

/** 读取根目录说明书；空文件、缺失或读失败都返回 None，不中断 Agent 回合。 */
pub fn load_project_instruction(root: &Path) -> Option<LoadedProjectInstruction> {
    let path = discover_project_instruction_file(root)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME)
        .to_owned();
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            log::warn!(
                target: "agent_runtime",
                "项目级 Agent 指令读取失败：path_chars={} error={}",
                path.to_string_lossy().chars().count(),
                crate::model_provider::redact_model_error_text(&error.to_string())
            );
            return None;
        }
    };
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    let truncated = trimmed.chars().count() > MAX_PROJECT_INSTRUCTION_CHARS;
    let bounded = if truncated {
        let cut: String = trimmed
            .chars()
            .take(MAX_PROJECT_INSTRUCTION_CHARS)
            .collect();
        format!("{cut}\n\n[内容已按上下文预算截断]")
    } else {
        trimmed.to_owned()
    };

    Some(LoadedProjectInstruction {
        file_name,
        content: bounded,
        truncated,
    })
}

/** 在知识库根目录创建标准 AGENTS.md；已有说明书（含大小写变体或旧名）时拒绝覆盖。 */
pub fn create_project_instruction_file(root: &Path) -> Result<String, String> {
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    if let Some(existing) = discover_project_instruction_file(&canonical_root) {
        let existing_name = existing
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME);
        return Err(format!("项目说明书已存在（{existing_name}），已阻止覆盖。"));
    }

    let target_path = canonical_root.join(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME);
    atomic_write_markdown(&target_path, PROJECT_INSTRUCTION_TEMPLATE)?;
    Ok(PROJECT_INSTRUCTION_CANONICAL_FILE_NAME.to_owned())
}

/** 新建说明书时使用的模板正文，供命令层写入笔记快照。 */
pub fn project_instruction_template() -> &'static str {
    PROJECT_INSTRUCTION_TEMPLATE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn root_path_detects_canonical_and_legacy_names() {
        assert!(is_root_project_instruction_path("AGENTS.md"));
        assert!(is_root_project_instruction_path("agents.md"));
        assert!(is_root_project_instruction_path("ORANGE_AGENT.md"));
        assert!(is_root_project_instruction_path("orange_agent.md"));
        assert!(!is_root_project_instruction_path("Notes/AGENTS.md"));
        assert!(!is_root_project_instruction_path("README.md"));
        assert!(!is_root_project_instruction_path(""));
    }

    #[test]
    fn discover_prefers_agents_md_over_legacy() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ORANGE_AGENT.md"), "旧规则").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "新规则").unwrap();

        let discovered = discover_project_instruction_file(dir.path()).unwrap();
        assert_eq!(
            discovered.file_name().and_then(|value| value.to_str()),
            Some("AGENTS.md")
        );

        let loaded = load_project_instruction(dir.path()).unwrap();
        assert_eq!(loaded.file_name, "AGENTS.md");
        assert_eq!(loaded.content, "新规则");
        assert!(!loaded.truncated);
    }

    #[test]
    fn discover_falls_back_to_legacy_orange_agent() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ORANGE_AGENT.md"), "兼容规则").unwrap();

        let loaded = load_project_instruction(dir.path()).unwrap();
        assert_eq!(loaded.file_name, "ORANGE_AGENT.md");
        assert_eq!(loaded.content, "兼容规则");
    }

    #[test]
    fn discover_accepts_case_variant_file_name() {
        let dir = tempdir().unwrap();
        let variant_name = if cfg!(windows) {
            // NTFS 大小写不敏感：用 AGENTS.md 写入后再按发现结果核对真实名字。
            fs::write(dir.path().join("AGENTS.md"), "windows").unwrap();
            discover_project_instruction_file(dir.path())
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        } else {
            fs::write(dir.path().join("Agents.md"), "unix-case").unwrap();
            "Agents.md".to_owned()
        };

        let loaded = load_project_instruction(dir.path()).unwrap();
        assert!(loaded.file_name.eq_ignore_ascii_case("AGENTS.md"));
        assert!(!variant_name.is_empty());
    }

    #[test]
    fn empty_or_missing_instruction_is_skipped() {
        let dir = tempdir().unwrap();
        assert!(load_project_instruction(dir.path()).is_none());

        fs::write(dir.path().join("AGENTS.md"), "   \n").unwrap();
        assert!(load_project_instruction(dir.path()).is_none());
    }

    #[test]
    fn nested_agents_md_is_not_discovered() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("Notes")).unwrap();
        fs::write(dir.path().join("Notes").join("AGENTS.md"), "子目录规则").unwrap();

        assert!(discover_project_instruction_file(dir.path()).is_none());
        assert!(load_project_instruction(dir.path()).is_none());
    }

    #[test]
    fn load_truncates_over_budget_content() {
        let dir = tempdir().unwrap();
        let oversized = "规".repeat(MAX_PROJECT_INSTRUCTION_CHARS + 32);
        fs::write(dir.path().join("AGENTS.md"), &oversized).unwrap();

        let loaded = load_project_instruction(dir.path()).unwrap();
        assert!(loaded.truncated);
        assert!(loaded.content.contains("[内容已按上下文预算截断]"));
        assert!(loaded.content.chars().count() < oversized.chars().count());
    }

    #[test]
    fn create_writes_template_and_rejects_overwrite() {
        let dir = tempdir().unwrap();
        let relative = create_project_instruction_file(dir.path()).unwrap();
        assert_eq!(relative, "AGENTS.md");

        let loaded = load_project_instruction(dir.path()).unwrap();
        assert!(loaded.content.contains("这份文件是给橘记 Agent 的项目规则"));

        let error = create_project_instruction_file(dir.path()).unwrap_err();
        assert!(error.contains("已存在"));
    }

    #[test]
    fn create_rejects_legacy_file_as_existing() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ORANGE_AGENT.md"), "旧").unwrap();
        let error = create_project_instruction_file(dir.path()).unwrap_err();
        assert!(error.contains("ORANGE_AGENT.md"));
    }
}
