use crate::domain::{AgentSecurityLevel, KnowledgeBase, EXTERNAL_FILESYSTEM_SCOPE_ID};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

/** Agent 路径解析结果：知识库内相对路径，或完全级别下的合规外部绝对路径。 */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFsTarget {
    pub knowledge_base_id: String,
    pub stored_path: String,
    pub absolute_path: PathBuf,
}

impl ResolvedFsTarget {
    pub fn is_external(&self) -> bool {
        self.knowledge_base_id == EXTERNAL_FILESYSTEM_SCOPE_ID
    }
}

/** 按会话级别解析 Agent 文件工具路径；相对路径始终落在授权知识库内。 */
pub fn resolve_agent_fs_target(
    knowledge_bases: &[KnowledgeBase],
    session_knowledge_base_ids: &[String],
    active_knowledge_base_id: &str,
    security_level: &str,
    requested_knowledge_base_id: Option<&str>,
    raw_path: &str,
    must_exist: bool,
) -> Result<ResolvedFsTarget, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err("目标路径不能为空。".to_owned());
    }

    let level = AgentSecurityLevel::parse(security_level);
    let expanded = if level.allows_external_filesystem() {
        expand_user_path(trimmed)
    } else {
        trimmed.to_owned()
    };
    let path = Path::new(&expanded);
    let session_ids = session_knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();

    let resolved = if path.is_absolute() {
        if !level.allows_external_filesystem() {
            return Err(
                "当前不是完全级别，只能使用知识库根目录内的相对路径；绝对路径已拒绝。".to_owned(),
            );
        }
        resolve_absolute_compliant_target(knowledge_bases, &session_ids, path)?
    } else {
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("目标路径超出知识库根目录，已阻止访问。".to_owned());
        }
        let knowledge_base = select_knowledge_base(
            knowledge_bases,
            session_knowledge_base_ids,
            &session_ids,
            active_knowledge_base_id,
            requested_knowledge_base_id,
        )?;
        resolve_relative_inside_knowledge_base(knowledge_base, path)?
    };

    if must_exist && !resolved.absolute_path.exists() {
        return Err(format!("目标路径不存在：{}", resolved.stored_path));
    }
    Ok(resolved)
}

/** 应用阶段再次解析已持久化的操作路径，不信任模型或前端改过的 payload。 */
pub fn resolve_persisted_operation_target(
    knowledge_bases: &[KnowledgeBase],
    session_knowledge_base_ids: &[String],
    security_level: &str,
    knowledge_base_id: &str,
    stored_path: &str,
) -> Result<PathBuf, String> {
    if knowledge_base_id == EXTERNAL_FILESYSTEM_SCOPE_ID {
        if !AgentSecurityLevel::parse(security_level).allows_external_filesystem() {
            return Err("当前会话已不再是完全级别，已拒绝知识库外路径。".to_owned());
        }
        let path = Path::new(stored_path);
        if !path.is_absolute() {
            return Err("外部路径必须是绝对路径。".to_owned());
        }
        let resolved = resolve_absolute_compliant_target(
            knowledge_bases,
            &session_knowledge_base_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            path,
        )?;
        return Ok(resolved.absolute_path);
    }

    let session_ids = session_knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if !session_ids.contains(knowledge_base_id) {
        return Err(format!("变更集操作指向未授权知识库：{knowledge_base_id}。"));
    }
    let knowledge_base = knowledge_bases
        .iter()
        .find(|item| item.id == knowledge_base_id)
        .ok_or_else(|| "变更集包含失效知识库。".to_owned())?;
    Ok(
        resolve_relative_inside_knowledge_base(knowledge_base, Path::new(stored_path))?
            .absolute_path,
    )
}

fn select_knowledge_base<'a>(
    knowledge_bases: &'a [KnowledgeBase],
    session_knowledge_base_ids: &[String],
    session_ids: &HashSet<&str>,
    active_knowledge_base_id: &str,
    requested_knowledge_base_id: Option<&str>,
) -> Result<&'a KnowledgeBase, String> {
    let knowledge_base_id = if let Some(requested) = requested_knowledge_base_id {
        if !session_ids.contains(requested) {
            return Err("目标知识库不在当前会话允许范围内，已拒绝。".to_owned());
        }
        requested
    } else if session_ids.contains(active_knowledge_base_id) {
        active_knowledge_base_id
    } else {
        session_knowledge_base_ids
            .iter()
            .map(String::as_str)
            .find(|id| session_ids.contains(id))
            .ok_or_else(|| "当前会话没有可用的授权知识库，已拒绝。".to_owned())?
    };

    knowledge_bases
        .iter()
        .find(|item| item.id == knowledge_base_id)
        .ok_or_else(|| "目标知识库不存在，已拒绝。".to_owned())
}

fn resolve_relative_inside_knowledge_base(
    knowledge_base: &KnowledgeBase,
    relative: &Path,
) -> Result<ResolvedFsTarget, String> {
    let canonical_root = fs::canonicalize(&knowledge_base.path)
        .map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let target = join_without_escape(&canonical_root, relative)?;
    let mut existing_ancestor = target
        .parent()
        .ok_or_else(|| "目标路径缺少父目录。".to_owned())?;
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor
            .parent()
            .ok_or_else(|| "无法解析目标父目录。".to_owned())?;
    }
    let canonical_ancestor = fs::canonicalize(existing_ancestor)
        .map_err(|error| format!("无法解析目标父目录：{error}"))?;
    if !canonical_ancestor.starts_with(&canonical_root) {
        return Err("目标路径超出知识库根目录，已阻止访问。".to_owned());
    }

    Ok(ResolvedFsTarget {
        knowledge_base_id: knowledge_base.id.clone(),
        stored_path: portable_path(relative).trim_end_matches('/').to_owned(),
        absolute_path: target,
    })
}

fn resolve_absolute_compliant_target(
    knowledge_bases: &[KnowledgeBase],
    session_ids: &HashSet<&str>,
    path: &Path,
) -> Result<ResolvedFsTarget, String> {
    let absolute = materialize_absolute_path(path)?;
    assert_compliant_path(&absolute)?;

    if let Some((knowledge_base, root)) =
        find_containing_knowledge_base(knowledge_bases, session_ids, &absolute)?
    {
        let relative = absolute
            .strip_prefix(&root)
            .map_err(|_| "无法计算知识库相对路径。".to_owned())?;
        return Ok(ResolvedFsTarget {
            knowledge_base_id: knowledge_base.id.clone(),
            stored_path: portable_path(relative).trim_end_matches('/').to_owned(),
            absolute_path: absolute,
        });
    }

    Ok(ResolvedFsTarget {
        knowledge_base_id: EXTERNAL_FILESYSTEM_SCOPE_ID.to_owned(),
        stored_path: portable_path(&absolute),
        absolute_path: absolute,
    })
}

fn find_containing_knowledge_base<'a>(
    knowledge_bases: &'a [KnowledgeBase],
    session_ids: &HashSet<&str>,
    absolute: &Path,
) -> Result<Option<(&'a KnowledgeBase, PathBuf)>, String> {
    let mut best: Option<(&'a KnowledgeBase, PathBuf)> = None;
    let mut blocked: Option<String> = None;

    for knowledge_base in knowledge_bases {
        let Ok(root) = fs::canonicalize(&knowledge_base.path) else {
            continue;
        };
        if absolute == root.as_path() || absolute.starts_with(&root) {
            if !session_ids.contains(knowledge_base.id.as_str()) {
                blocked = Some(knowledge_base.id.clone());
                continue;
            }
            let longer = best
                .as_ref()
                .is_none_or(|(_, current)| root.as_os_str().len() >= current.as_os_str().len());
            if longer {
                best = Some((knowledge_base, root));
            }
        }
    }

    if best.is_none() {
        if let Some(knowledge_base_id) = blocked {
            return Err(format!(
                "目标路径位于当前会话未授权的知识库中：{knowledge_base_id}。"
            ));
        }
    }
    Ok(best)
}

/** 把尚未存在的绝对路径展开为“已存在祖先的规范路径 + 剩余分段”。 */
fn materialize_absolute_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("目标路径不是绝对路径。".to_owned());
    }

    let mut skipped = Vec::new();
    let mut ancestor = path;
    loop {
        if ancestor.exists() {
            break;
        }
        match ancestor.file_name() {
            Some(name) => {
                skipped.push(name.to_os_string());
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| "无法解析目标路径。".to_owned())?;
            }
            None => return Err("无法解析目标路径。".to_owned()),
        }
    }

    let mut target =
        fs::canonicalize(ancestor).map_err(|error| format!("无法解析目标路径：{error}"))?;
    for name in skipped.into_iter().rev() {
        target.push(name);
    }
    Ok(target)
}

fn assert_compliant_path(path: &Path) -> Result<(), String> {
    if has_forbidden_component(path) {
        return Err("目标路径包含受保护的系统目录，已拒绝。".to_owned());
    }

    let mut probe = path;
    while !probe.exists() {
        probe = probe
            .parent()
            .ok_or_else(|| "无法解析目标路径。".to_owned())?;
    }
    let canonical =
        fs::canonicalize(probe).map_err(|error| format!("无法解析目标路径：{error}"))?;
    if is_under_protected_root(&canonical) {
        return Err("目标路径位于受保护的系统目录，已拒绝。".to_owned());
    }
    Ok(())
}

fn join_without_escape(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut target = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => target.push(name),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("目标路径超出知识库根目录，已阻止访问。".to_owned());
            }
        }
    }
    Ok(target)
}

fn has_forbidden_component(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => {
            let value = name.to_string_lossy();
            value.eq_ignore_ascii_case("$Recycle.Bin")
                || value.eq_ignore_ascii_case("System Volume Information")
        }
        _ => false,
    })
}

fn is_under_protected_root(path: &Path) -> bool {
    protected_system_roots()
        .iter()
        .any(|root| path == root.as_path() || path.starts_with(root))
}

fn protected_system_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push_env = |key: &str| {
        if let Ok(value) = std::env::var(key) {
            if let Ok(canonical) = fs::canonicalize(PathBuf::from(value)) {
                if !roots.iter().any(|existing| existing == &canonical) {
                    roots.push(canonical);
                }
            }
        }
    };

    #[cfg(windows)]
    {
        push_env("windir");
        push_env("SystemRoot");
        push_env("ProgramFiles");
        push_env("ProgramFiles(x86)");
        push_env("ProgramW6432");
    }

    #[cfg(unix)]
    {
        for candidate in [
            "/bin",
            "/sbin",
            "/usr",
            "/etc",
            "/dev",
            "/proc",
            "/sys",
            "/System",
            "/Library",
            "/private/etc",
        ] {
            if let Ok(canonical) = fs::canonicalize(candidate) {
                if !roots.iter().any(|existing| existing == &canonical) {
                    roots.push(canonical);
                }
            }
        }
    }

    let _ = &mut push_env;
    roots
}

fn expand_user_path(raw: &str) -> String {
    if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        if let Some(home) = home_dir() {
            if raw.len() == 1 {
                return home.to_string_lossy().into_owned();
            }
            return home.join(&raw[2..]).to_string_lossy().into_owned();
        }
    }
    raw.to_owned()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn portable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::KnowledgeBase;

    fn knowledge_base(id: &str, path: &Path) -> KnowledgeBase {
        KnowledgeBase {
            id: id.to_owned(),
            name: id.to_owned(),
            path: path.to_string_lossy().into_owned(),
            description: String::new(),
            status: "ready".to_owned(),
            note_count: 0,
            document_count: 0,
            updated_at: "t".to_owned(),
            is_default: id == "kb-a",
            semantic_index_enabled: false,
            scan_report: None,
        }
    }

    #[test]
    fn advanced_mode_rejects_absolute_and_escape_paths() {
        let root = tempfile::tempdir().expect("kb root");
        let kb = knowledge_base("kb-a", root.path());
        let session_ids = vec!["kb-a".to_owned()];

        let escaped = resolve_agent_fs_target(
            &[kb.clone()],
            &session_ids,
            "kb-a",
            "advanced",
            None,
            "../escape",
            false,
        );
        assert!(escaped.is_err());

        let absolute = resolve_agent_fs_target(
            &[kb],
            &session_ids,
            "kb-a",
            "advanced",
            None,
            &root.path().join("outside-sibling").to_string_lossy(),
            false,
        );
        assert!(absolute.unwrap_err().contains("完全级别"));
    }

    #[test]
    fn advanced_mode_allows_relative_path_inside_kb() {
        let root = tempfile::tempdir().expect("kb root");
        let kb = knowledge_base("kb-a", root.path());
        let resolved = resolve_agent_fs_target(
            &[kb],
            &["kb-a".to_owned()],
            "kb-a",
            "advanced",
            None,
            "Notes/新目录",
            false,
        )
        .expect("relative path");

        assert_eq!(resolved.knowledge_base_id, "kb-a");
        assert_eq!(resolved.stored_path, "Notes/新目录");
        assert!(resolved
            .absolute_path
            .ends_with(Path::new("Notes").join("新目录")));
    }

    #[test]
    fn full_mode_maps_absolute_path_inside_kb() {
        let root = tempfile::tempdir().expect("kb root");
        fs::create_dir_all(root.path().join("Notes")).expect("notes dir");
        let kb = knowledge_base("kb-a", root.path());
        let absolute = root.path().join("Notes").join("归档");

        let resolved = resolve_agent_fs_target(
            &[kb],
            &["kb-a".to_owned()],
            "kb-a",
            "autonomous",
            None,
            &absolute.to_string_lossy(),
            false,
        )
        .expect("map into kb");

        assert_eq!(resolved.knowledge_base_id, "kb-a");
        assert_eq!(resolved.stored_path, "Notes/归档");
        assert!(!resolved.is_external());
    }

    #[test]
    fn full_mode_allows_compliant_external_absolute_path() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let external = tempfile::tempdir().expect("external root");
        let kb = knowledge_base("kb-a", kb_root.path());
        let target = external.path().join("AgentOut").join("docs");

        let resolved = resolve_agent_fs_target(
            &[kb],
            &["kb-a".to_owned()],
            "kb-a",
            "autonomous",
            None,
            &target.to_string_lossy(),
            false,
        )
        .expect("external path");

        assert!(resolved.is_external());
        assert_eq!(resolved.knowledge_base_id, EXTERNAL_FILESYSTEM_SCOPE_ID);
        assert!(resolved.stored_path.contains("AgentOut"));
    }

    #[test]
    fn full_mode_rejects_unscoped_knowledge_base_absolute_path() {
        let scoped = tempfile::tempdir().expect("scoped kb");
        let other = tempfile::tempdir().expect("other kb");
        let knowledge_bases = vec![
            knowledge_base("kb-a", scoped.path()),
            knowledge_base("kb-b", other.path()),
        ];
        let target = other.path().join("secret");

        let error = resolve_agent_fs_target(
            &knowledge_bases,
            &["kb-a".to_owned()],
            "kb-a",
            "autonomous",
            None,
            &target.to_string_lossy(),
            false,
        )
        .unwrap_err();
        assert!(error.contains("未授权的知识库"));
    }

    #[cfg(windows)]
    #[test]
    fn full_mode_rejects_windows_directory() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let kb = knowledge_base("kb-a", kb_root.path());
        let windir = std::env::var("windir").unwrap_or_else(|_| r"C:\Windows".to_owned());
        let target = PathBuf::from(windir).join("orange-not-allowed");

        let error = resolve_agent_fs_target(
            &[kb],
            &["kb-a".to_owned()],
            "kb-a",
            "autonomous",
            None,
            &target.to_string_lossy(),
            false,
        )
        .unwrap_err();
        assert!(error.contains("受保护"));
    }

    #[test]
    fn persisted_external_path_requires_full_mode() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let external = tempfile::tempdir().expect("external root");
        let kb = knowledge_base("kb-a", kb_root.path());
        let stored = portable_path(&external.path().join("docs"));

        let denied = resolve_persisted_operation_target(
            &[kb.clone()],
            &["kb-a".to_owned()],
            "advanced",
            EXTERNAL_FILESYSTEM_SCOPE_ID,
            &stored,
        );
        assert!(denied.is_err());

        let allowed = resolve_persisted_operation_target(
            &[kb],
            &["kb-a".to_owned()],
            "autonomous",
            EXTERNAL_FILESYSTEM_SCOPE_ID,
            &stored,
        );
        assert!(allowed.is_ok());
    }
}
