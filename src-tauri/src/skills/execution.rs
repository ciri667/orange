use super::load_agent_skills;
use crate::domain::{
    ProposedChangeSet, ProposedFileOperation, SkillExecutionRequest, WorkspaceSnapshot,
    AGENT_DIRECT_EXECUTION_ID, EXTERNAL_FILESYSTEM_SCOPE_ID,
};
use crate::storage;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

/** 单次 Skill 输出最多进入可见诊断的字符数，避免脚本输出正文或密钥淹没日志。 */
const MAX_EXECUTION_OUTPUT_CHARS: usize = 2000;

/** 审批后在隔离副本中运行 Skill，并将文件系统差异转换为待确认变更集。 */
pub fn approve_and_execute(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let request = snapshot.sessions[session_index]
        .pending_execution
        .clone()
        .filter(|request| request.status == "pending")
        .ok_or_else(|| "当前会话没有待确认的 Skill 执行。".to_owned())?;
    validate_storage_identifier(&request.id, "执行 ID")?;
    for knowledge_base_id in &request.knowledge_base_ids {
        validate_storage_identifier(knowledge_base_id, "知识库 ID")?;
    }
    let settings = storage::load_user_settings(app)?;

    if !settings.agent_security.advanced_execution_enabled
        || snapshot.sessions[session_index].security_level == "basic"
    {
        return Err("当前 Agent 权限设置不允许执行 Skill。".to_owned());
    }
    if !request.network_domains.is_empty() {
        return Err("Windows 首版尚未开放受控网络代理，声明联网的 Skill 已拒绝执行。".to_owned());
    }
    if !request.credential_aliases.is_empty() {
        return Err("Windows 首版尚未开放凭证注入，声明凭证的 Skill 已拒绝执行。".to_owned());
    }

    let connection = storage::open_database(app)?;
    let skill = load_agent_skills(app, &connection)?
        .into_iter()
        .find(|skill| skill.id == request.skill_id && skill.enabled)
        .ok_or_else(|| "找不到待执行的已启用 Skill。".to_owned())?;
    let manifest = skill
        .runtime_manifest
        .as_ref()
        .ok_or_else(|| "该 Skill 没有可执行入口。".to_owned())?;
    let compatibility = skill
        .compatibility
        .as_ref()
        .ok_or_else(|| "无法确认 Skill 兼容性。".to_owned())?;
    if compatibility.status != "ready" || compatibility.package_hash != request.package_hash {
        return Err("Skill 包已变化或当前不兼容，请重新发起执行请求。".to_owned());
    }

    let skill_markdown_path = PathBuf::from(
        skill
            .path
            .as_deref()
            .ok_or_else(|| "内置指令 Skill 不支持本地执行。".to_owned())?,
    );
    let skill_dir = skill_markdown_path
        .parent()
        .ok_or_else(|| "无法解析 Skill 目录。".to_owned())?;
    let runtime_status = compatibility
        .runtime
        .as_ref()
        .filter(|runtime| runtime.available)
        .ok_or_else(|| "Skill 运行时不可用。".to_owned())?;
    let runtime_path = PathBuf::from(
        runtime_status
            .executable_path
            .as_deref()
            .ok_or_else(|| "Skill 运行时路径缺失。".to_owned())?,
    );

    let run_root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法解析应用数据目录：{error}"))?
        .join("skill-runs")
        .join(&request.id);
    let workspace_root = run_root.join("workspace");
    let baseline_root = run_root.join("baseline");
    if run_root.exists() {
        fs::remove_dir_all(&run_root)
            .map_err(|error| format!("无法重置 Skill 隔离工作区：{error}"))?;
    }
    fs::create_dir_all(&workspace_root)
        .map_err(|error| format!("无法创建 Skill 隔离工作区：{error}"))?;
    let mut roots = copy_scope_to_workspace(&snapshot, &request, &baseline_root)?;
    copy_workspace_tree(&baseline_root, &workspace_root)?;
    for mapping in &mut roots {
        mapping.workspace_root = workspace_root.join(&mapping.knowledge_base_id);
    }

    run_in_windows_sandbox(
        &request,
        manifest,
        skill_dir,
        &runtime_path,
        &workspace_root,
        &settings.agent_security.resource_limits,
    )?;
    let change_set = build_change_set(
        &request,
        &skill.id,
        &workspace_root,
        &roots,
        settings.agent_security.resource_limits.max_artifact_mb,
    )?;

    snapshot.sessions[session_index].pending_execution = Some(SkillExecutionRequest {
        status: "completed".to_owned(),
        ..request
    });
    snapshot.sessions[session_index].pending_change_set = Some(change_set);
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    if snapshot.sessions[session_index].security_level == "autonomous" {
        storage::save_trusted_skill_grant(
            app,
            crate::domain::TrustedSkillGrant {
                skill_id: skill.id.clone(),
                package_hash: compatibility.package_hash.clone(),
                expires_at: Some((chrono::Utc::now() + chrono::Duration::minutes(30)).to_rfc3339()),
            },
        )?;
    }
    storage::save_sessions(app, &snapshot)?;

    Ok(snapshot)
}

/** 完全级别仅对同一 Skill 包 hash 的未过期短时授权自动执行。 */
pub fn can_auto_execute(
    session: &crate::domain::AgentSession,
    request: &SkillExecutionRequest,
    settings: &crate::domain::AgentSecuritySettings,
) -> bool {
    if !crate::agent_writes::allows_autonomous_auto_apply(session, settings) {
        return false;
    }

    settings.trusted_skill_grants.iter().any(|grant| {
        grant.skill_id == request.skill_id
            && grant.package_hash == request.package_hash
            && grant.expires_at.as_deref().is_some_and(|expires_at| {
                chrono::DateTime::parse_from_rfc3339(expires_at)
                    .map(|expires_at| expires_at.with_timezone(&chrono::Utc) > chrono::Utc::now())
                    .unwrap_or(false)
            })
    })
}

/** 拒绝待审批执行，不创建工作区、不启动任何进程。 */
pub fn reject_execution(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let request = snapshot.sessions[session_index]
        .pending_execution
        .clone()
        .filter(|request| request.status == "pending")
        .ok_or_else(|| "当前会话没有待确认的 Skill 执行。".to_owned())?;

    snapshot.sessions[session_index].pending_execution = Some(SkillExecutionRequest {
        status: "rejected".to_owned(),
        ..request
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::save_sessions(app, &snapshot)?;
    Ok(snapshot)
}

/** 应用变更集中所有已选操作；先全量预检，写入中途失败则按备份逆序回滚。 */
pub fn apply_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let change_set = snapshot.sessions[session_index]
        .pending_change_set
        .clone()
        .filter(|change_set| change_set.status == "pending")
        .ok_or_else(|| "当前会话没有待确认的 Skill 变更集。".to_owned())?;
    let request = snapshot.sessions[session_index]
        .pending_execution
        .clone()
        .filter(|request| request.id == change_set.execution_id && request.status == "completed")
        .ok_or_else(|| "找不到与变更集匹配的已完成执行。".to_owned())?;
    validate_storage_identifier(&change_set.execution_id, "执行 ID")?;
    for knowledge_base_id in &request.knowledge_base_ids {
        validate_storage_identifier(knowledge_base_id, "知识库 ID")?;
    }
    let selected_ids = change_set
        .operations
        .iter()
        .filter(|operation| operation.selected)
        .map(|operation| operation.id.as_str())
        .collect::<HashSet<_>>();
    let run_root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法解析应用数据目录：{error}"))?
        .join("skill-runs")
        .join(&change_set.execution_id);
    let baseline_root = run_root.join("baseline");
    let workspace_root = run_root.join("workspace");
    let mappings = request
        .knowledge_base_ids
        .iter()
        .map(|knowledge_base_id| WorkspaceRootMapping {
            knowledge_base_id: knowledge_base_id.clone(),
            source_root: baseline_root.join(knowledge_base_id),
            workspace_root: workspace_root.join(knowledge_base_id),
        })
        .collect::<Vec<_>>();
    let max_artifact_mb = storage::load_user_settings(app)?
        .agent_security
        .resource_limits
        .max_artifact_mb;
    let mut trusted_change_set = build_change_set(
        &request,
        &change_set.skill_id,
        &workspace_root,
        &mappings,
        max_artifact_mb,
    )?;
    for operation in &mut trusted_change_set.operations {
        operation.selected = selected_ids.contains(operation.id.as_str());
    }
    let change_set = ProposedChangeSet {
        id: change_set.id,
        status: change_set.status,
        created_at: change_set.created_at,
        ..trusted_change_set
    };
    let selected = change_set
        .operations
        .iter()
        .filter(|operation| operation.selected)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("变更集中没有已选择的文件操作。".to_owned());
    }

    let knowledge_bases = snapshot
        .knowledge_bases
        .iter()
        .map(|knowledge_base| (knowledge_base.id.as_str(), knowledge_base.path.as_str()))
        .collect::<HashMap<_, _>>();
    let mut prepared: Vec<(ProposedFileOperation, PathBuf, RollbackAction)> = Vec::new();
    for operation in selected {
        let root = Path::new(
            knowledge_bases
                .get(operation.knowledge_base_id.as_str())
                .copied()
                .ok_or_else(|| "变更集包含失效知识库。".to_owned())?,
        );
        let target = resolve_target_without_creation(root, &operation.target_path)?;
        let existing = target.exists();
        match operation.operation.as_str() {
            "create" if existing => {
                return Err(format!("目标文件已存在：{}。", operation.target_path))
            }
            "modify" | "delete" if !existing => {
                return Err(format!("目标文件已不存在：{}。", operation.target_path))
            }
            // create_folder 允许目标已存在（幂等），但既有二进制路径拒绝落入普通文件。
            "create_folder" if existing && target.is_file() => {
                return Err(format!("目标路径已存在文件：{}。", operation.target_path))
            }
            "create" | "modify" | "delete" | "create_folder" => {}
            _ => return Err(format!("不支持的文件操作：{}。", operation.operation)),
        }
        if operation.operation == "create_folder" {
            // 文件夹新建幂等：已存在视为无需写入，回滚动作也不应删除既有目录。
            let rollback = if existing {
                RollbackAction::None
            } else {
                RollbackAction::RemoveCreated
            };
            prepared.push((operation.clone(), target, rollback));
        } else if existing {
            let bytes = fs::read(&target).map_err(|error| format!("无法读取目标文件：{error}"))?;
            if operation.binary {
                return Err("既有二进制文件不能由 Skill 修改或删除。".to_owned());
            }
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| format!("目标文件不是有效 UTF-8 文本：{}。", operation.target_path))?;
            if storage::hash_content(&text) != operation.original_hash {
                return Err(format!(
                    "目标文件已变化，已阻止写入：{}。",
                    operation.target_path
                ));
            }
            prepared.push((operation.clone(), target, RollbackAction::Restore(bytes)));
        } else {
            prepared.push((operation.clone(), target, RollbackAction::RemoveCreated));
        }
    }

    let mut applied: Vec<(PathBuf, RollbackAction)> = Vec::new();
    for (operation, target, rollback) in &prepared {
        let result = apply_file_operation(operation, target);
        if let Err(error) = result {
            rollback_files(&applied);
            return Err(format!("应用变更失败并已回滚：{error}"));
        }
        applied.push((target.clone(), rollback.clone()));
    }

    let affected_ids = prepared
        .iter()
        .map(|(operation, _, _)| operation.knowledge_base_id.clone())
        .collect::<HashSet<_>>();
    refresh_affected_knowledge_bases(&mut snapshot, &affected_ids)?;
    normalize_active_file_after_change_set(&mut snapshot);
    let execution_id = change_set.execution_id.clone();
    snapshot.sessions[session_index].pending_change_set = Some(ProposedChangeSet {
        status: "applied".to_owned(),
        ..change_set
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::index_snapshot(app, &snapshot)?;
    storage::save_sessions(app, &snapshot)?;
    // 文件已经安全落盘后，隔离区清理失败只保留待下次启动清理，不把成功应用误报为失败。
    let _ = cleanup_run_directory(app, &execution_id);
    Ok(snapshot)
}

/** 删除或移动当前文件后选择同知识库内的可用文件，避免前端保留失效焦点。 */
fn normalize_active_file_after_change_set(snapshot: &mut WorkspaceSnapshot) {
    if !snapshot.active_note_id.is_empty()
        && snapshot
            .notes
            .iter()
            .any(|note| note.id == snapshot.active_note_id)
    {
        snapshot.active_document_id.clear();
        return;
    }
    if !snapshot.active_document_id.is_empty()
        && snapshot
            .documents
            .iter()
            .any(|document| document.id == snapshot.active_document_id)
    {
        snapshot.active_note_id.clear();
        return;
    }

    snapshot.active_note_id = snapshot
        .notes
        .iter()
        .find(|note| note.knowledge_base_id == snapshot.active_knowledge_base_id)
        .map(|note| note.id.clone())
        .unwrap_or_default();
    snapshot.active_document_id = if snapshot.active_note_id.is_empty() {
        snapshot
            .documents
            .iter()
            .find(|document| document.knowledge_base_id == snapshot.active_knowledge_base_id)
            .map(|document| document.id.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
}

/** 拒绝整个变更集并清理对应隔离任务目录；真实知识库保持不变。 */
pub fn reject_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let change_set = snapshot.sessions[session_index]
        .pending_change_set
        .clone()
        .filter(|change_set| change_set.status == "pending")
        .ok_or_else(|| "当前会话没有待确认的 Skill 变更集。".to_owned())?;
    validate_storage_identifier(&change_set.execution_id, "执行 ID")?;
    cleanup_run_directory(app, &change_set.execution_id)?;
    snapshot.sessions[session_index].pending_change_set = Some(ProposedChangeSet {
        status: "rejected".to_owned(),
        ..change_set
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::save_sessions(app, &snapshot)?;
    Ok(snapshot)
}

/** 应用 Agent 直接产生的变更集（无 Skill 执行隔离区）；仍走可信快照重载、全量预检、原子写入与回滚。 */
pub fn apply_agent_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let change_set = snapshot.sessions[session_index]
        .pending_change_set
        .clone()
        .filter(|change_set| {
            change_set.status == "pending" && change_set.execution_id == AGENT_DIRECT_EXECUTION_ID
        })
        .ok_or_else(|| "当前会话没有待确认的 Agent 变更集。".to_owned())?;

    let selected = change_set
        .operations
        .iter()
        .filter(|operation| operation.selected)
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("变更集中没有已选择的文件操作。".to_owned());
    }

    let mut prepared: Vec<(ProposedFileOperation, PathBuf, RollbackAction)> = Vec::new();
    for operation in &selected {
        let target = crate::fs_guard::resolve_persisted_operation_target(
            &snapshot.knowledge_bases,
            &snapshot.sessions[session_index].knowledge_base_ids,
            &snapshot.sessions[session_index].security_level,
            &operation.knowledge_base_id,
            &operation.target_path,
        )?;
        let existing = target.exists();
        match operation.operation.as_str() {
            "create_folder" if existing && target.is_file() => {
                return Err(format!("目标路径已存在文件：{}。", operation.target_path))
            }
            "create_folder" => {}
            _ => {
                return Err(format!(
                    "Agent 变更集暂只支持 create_folder 操作：{}。",
                    operation.operation
                ))
            }
        }
        let rollback = if existing {
            RollbackAction::None
        } else {
            RollbackAction::RemoveCreated
        };
        prepared.push((operation.clone(), target, rollback));
    }

    let mut applied: Vec<(PathBuf, RollbackAction)> = Vec::new();
    for (operation, target, rollback) in &prepared {
        if let Err(error) = apply_file_operation(operation, target) {
            rollback_files(&applied);
            return Err(format!("应用变更失败并已回滚：{error}"));
        }
        applied.push((target.clone(), rollback.clone()));
    }

    let affected_ids = prepared
        .iter()
        .map(|(operation, _, _)| operation.knowledge_base_id.clone())
        .filter(|knowledge_base_id| knowledge_base_id != EXTERNAL_FILESYSTEM_SCOPE_ID)
        .collect::<HashSet<_>>();
    refresh_affected_knowledge_bases(&mut snapshot, &affected_ids)?;
    normalize_active_file_after_change_set(&mut snapshot);
    snapshot.sessions[session_index].pending_change_set = Some(ProposedChangeSet {
        status: "applied".to_owned(),
        ..change_set
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::index_snapshot(app, &snapshot)?;
    storage::save_sessions(app, &snapshot)?;
    Ok(snapshot)
}

/** 拒绝 Agent 变更集；只清空待确认状态，不触碰 Skill 隔离目录。 */
pub fn reject_agent_change_set(
    app: &AppHandle,
    snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = hydrate_trusted_snapshot(app, snapshot)?;
    let session_index = active_local_session_index(&snapshot)?;
    let change_set = snapshot.sessions[session_index]
        .pending_change_set
        .clone()
        .filter(|change_set| {
            change_set.status == "pending" && change_set.execution_id == AGENT_DIRECT_EXECUTION_ID
        })
        .ok_or_else(|| "当前会话没有待确认的 Agent 变更集。".to_owned())?;
    snapshot.sessions[session_index].pending_change_set = Some(ProposedChangeSet {
        status: "rejected".to_owned(),
        ..change_set
    });
    snapshot.sessions[session_index].updated_at = storage::format_local_datetime();
    storage::save_sessions(app, &snapshot)?;
    Ok(snapshot)
}

/** 判断自主模式是否可对 Agent 直接产出的变更集自动应用。 */
pub fn can_auto_apply_agent_change_set(
    session: &crate::domain::AgentSession,
    settings: &crate::domain::AgentSecuritySettings,
) -> bool {
    if !crate::agent_writes::allows_autonomous_auto_apply(session, settings) {
        return false;
    }
    session
        .pending_change_set
        .as_ref()
        .is_some_and(|change_set| {
            change_set.status == "pending" && change_set.execution_id == AGENT_DIRECT_EXECUTION_ID
        })
}

/** 对单个已预检操作执行原子文本写入、新增二进制复制或创建文件夹。 */
fn apply_file_operation(operation: &ProposedFileOperation, target: &Path) -> Result<(), String> {
    match operation.operation.as_str() {
        "create_folder" => {
            fs::create_dir_all(target).map_err(|error| format!("无法创建文件夹：{error}"))
        }
        "delete" => fs::remove_file(target).map_err(|error| format!("无法删除文件：{error}")),
        "create" | "modify" if operation.binary => {
            let parent = target
                .parent()
                .ok_or_else(|| "目标路径缺少父目录。".to_owned())?;
            fs::create_dir_all(parent).map_err(|error| format!("无法创建目标父目录：{error}"))?;
            let staged = operation
                .staged_path
                .as_deref()
                .map(Path::new)
                .filter(|path| path.is_file())
                .ok_or_else(|| "找不到隔离区二进制产物。".to_owned())?;
            let temp = tempfile::NamedTempFile::new_in(parent)
                .map_err(|error| format!("无法创建二进制临时文件：{error}"))?;
            fs::copy(staged, temp.path())
                .map_err(|error| format!("无法复制二进制产物：{error}"))?;
            temp.persist(target)
                .map_err(|error| format!("无法写入二进制产物：{}", error.error))?;
            Ok(())
        }
        "create" | "modify" => {
            let parent = target
                .parent()
                .ok_or_else(|| "目标路径缺少父目录。".to_owned())?;
            fs::create_dir_all(parent).map_err(|error| format!("无法创建目标父目录：{error}"))?;
            let next = operation
                .next
                .as_deref()
                .ok_or_else(|| "文本变更缺少 next 内容。".to_owned())?;
            if operation.file_type == "markdown" {
                storage::atomic_write_markdown(target, next)
            } else {
                storage::atomic_write_text_document(target, next)
            }
        }
        _ => Err("不支持的文件操作。".to_owned()),
    }
}

/** 纯只读解析目标路径；验证阶段不创建目录，保证全量预检失败时知识库零变化。 */
fn resolve_target_without_creation(root: &Path, relative_path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative_path);
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("目标路径超出知识库根目录，已阻止写入。".to_owned());
    }
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("无法解析知识库根目录：{error}"))?;
    let target = canonical_root.join(relative);
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
        return Err("目标路径超出知识库根目录，已阻止写入。".to_owned());
    }
    Ok(target)
}

/** 清理已完成或已拒绝执行的隔离目录，避免本地副本长期占用空间。 */
fn cleanup_run_directory(app: &AppHandle, execution_id: &str) -> Result<(), String> {
    let run_root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法解析应用数据目录：{error}"))?
        .join("skill-runs")
        .join(execution_id);
    if run_root.exists() {
        fs::remove_dir_all(&run_root)
            .map_err(|error| format!("无法清理 Skill 隔离工作区：{error}"))?;
    }
    Ok(())
}

/** 单个已应用操作的回滚策略；文件夹新建回滚删除目录，文本改写回滚恢复原文。 */
#[derive(Clone)]
enum RollbackAction {
    /** 目标原本不存在，撤销时删除本次新建的文件或文件夹。 */
    RemoveCreated,
    /** 目标原本是文本文件，撤销时用备份字节覆盖回去。 */
    Restore(Vec<u8>),
    /** 本次应用是幂等无操作（如目录已存在），撤销时不需要做任何清理。 */
    None,
}

/** 尽力恢复已经写入的文件；RemoveCreated 表示该文件/文件夹原本不存在。 */
fn rollback_files(applied: &[(PathBuf, RollbackAction)]) {
    for (path, action) in applied.iter().rev() {
        match action {
            RollbackAction::RemoveCreated => {
                // 文件夹新建失败回滚时删目录，文件新建失败回滚时删文件；两者都尽力移除。
                let _ = fs::remove_file(path).or_else(|_| fs::remove_dir(path));
            }
            RollbackAction::Restore(bytes) => {
                let _ = fs::write(path, bytes);
            }
            RollbackAction::None => {}
        }
    }
}

/** 应用后重新扫描受影响知识库，使前端快照、FTS 和真实磁盘重新对齐。 */
fn refresh_affected_knowledge_bases(
    snapshot: &mut WorkspaceSnapshot,
    affected_ids: &HashSet<String>,
) -> Result<(), String> {
    for knowledge_base_id in affected_ids {
        let previous = snapshot
            .knowledge_bases
            .iter()
            .find(|knowledge_base| knowledge_base.id == *knowledge_base_id)
            .cloned()
            .ok_or_else(|| "找不到要刷新的知识库。".to_owned())?;
        let selection = crate::domain::KnowledgeBaseSelection {
            id: previous.id.clone(),
            name: previous.name.clone(),
            path: previous.path.clone(),
            note_count: previous.note_count,
        };
        let (mut knowledge_base, folders, notes, documents) =
            storage::scan_supported_documents_directory(&selection)?;
        knowledge_base.is_default = previous.is_default;
        knowledge_base.semantic_index_enabled = previous.semantic_index_enabled;
        if let Some(target) = snapshot
            .knowledge_bases
            .iter_mut()
            .find(|knowledge_base| knowledge_base.id == *knowledge_base_id)
        {
            *target = knowledge_base;
        }
        snapshot
            .folders
            .retain(|folder| folder.knowledge_base_id != *knowledge_base_id);
        snapshot
            .notes
            .retain(|note| note.knowledge_base_id != *knowledge_base_id);
        snapshot
            .documents
            .retain(|document| document.knowledge_base_id != *knowledge_base_id);
        snapshot.folders.extend(folders);
        snapshot.notes.extend(notes);
        snapshot.documents.extend(documents);
    }
    Ok(())
}

/** 高权限执行只允许本地会话，远程 IM 入口在此再次硬拒绝。 */
fn active_local_session_index(snapshot: &WorkspaceSnapshot) -> Result<usize, String> {
    let index = snapshot
        .sessions
        .iter()
        .position(|session| session.id == snapshot.active_session_id)
        .ok_or_else(|| "找不到当前 Agent 会话。".to_owned())?;
    if snapshot.sessions[index].im_identity.is_some() {
        return Err("即时通讯会话不允许执行本地 Skill。".to_owned());
    }
    Ok(index)
}

/** 审批和应用命令只信任 SQLite 中的知识库路径、会话 scope 和待处理载荷。 */
fn hydrate_trusted_snapshot(
    app: &AppHandle,
    client_snapshot: WorkspaceSnapshot,
) -> Result<WorkspaceSnapshot, String> {
    let requested_session_id = client_snapshot.active_session_id;
    let requested_knowledge_base_id = client_snapshot.active_knowledge_base_id;
    let requested_note_id = client_snapshot.active_note_id;
    let requested_document_id = client_snapshot.active_document_id;
    let mut snapshot = storage::load_workspace_snapshot(app)?;
    if !snapshot
        .sessions
        .iter()
        .any(|session| session.id == requested_session_id)
    {
        return Err("找不到持久化的 Agent 会话。".to_owned());
    }
    snapshot.active_session_id = requested_session_id;
    if snapshot
        .knowledge_bases
        .iter()
        .any(|knowledge_base| knowledge_base.id == requested_knowledge_base_id)
    {
        snapshot.active_knowledge_base_id = requested_knowledge_base_id;
    }
    if snapshot
        .notes
        .iter()
        .any(|note| note.id == requested_note_id)
    {
        snapshot.active_note_id = requested_note_id;
        snapshot.active_document_id.clear();
    } else if snapshot
        .documents
        .iter()
        .any(|document| document.id == requested_document_id)
    {
        snapshot.active_document_id = requested_document_id;
        snapshot.active_note_id.clear();
    }
    Ok(snapshot)
}

/** 目录片段只允许稳定 ASCII 标识符，拒绝路径分隔符、点目录和控制字符。 */
fn validate_storage_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || value == "."
        || value == ".."
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("{label} 非法，已拒绝执行。"));
    }
    Ok(())
}

/** 隔离副本映射记录真实知识库根和工作区根，用于执行后做确定性差异比较。 */
struct WorkspaceRootMapping {
    knowledge_base_id: String,
    source_root: PathBuf,
    workspace_root: PathBuf,
}

/** 将本轮授权知识库复制到隔离工作区；符号链接和生成目录不会进入副本。 */
fn copy_scope_to_workspace(
    snapshot: &WorkspaceSnapshot,
    request: &SkillExecutionRequest,
    workspace_root: &Path,
) -> Result<Vec<WorkspaceRootMapping>, String> {
    let allowed_ids = request
        .knowledge_base_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut mappings = Vec::new();

    for knowledge_base in snapshot
        .knowledge_bases
        .iter()
        .filter(|knowledge_base| allowed_ids.contains(knowledge_base.id.as_str()))
    {
        let source_root = fs::canonicalize(&knowledge_base.path)
            .map_err(|error| format!("无法读取授权知识库：{error}"))?;
        let target_root = workspace_root.join(&knowledge_base.id);
        fs::create_dir_all(&target_root)
            .map_err(|error| format!("无法创建知识库隔离副本：{error}"))?;

        for entry in WalkDir::new(&source_root).follow_links(false) {
            let entry = entry.map_err(|error| format!("无法复制知识库：{error}"))?;
            if entry.file_type().is_symlink() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&source_root)
                .map_err(|_| "无法解析知识库相对路径。".to_owned())?;
            if should_skip_relative_path(relative) {
                continue;
            }
            let target = target_root.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)
                    .map_err(|error| format!("无法创建隔离副本目录：{error}"))?;
            } else if entry.file_type().is_file() && is_supported_workspace_file(entry.path()) {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("无法创建隔离副本父目录：{error}"))?;
                }
                fs::copy(entry.path(), &target)
                    .map_err(|error| format!("无法复制知识库文件：{error}"))?;
            }
        }
        mappings.push(WorkspaceRootMapping {
            knowledge_base_id: knowledge_base.id.clone(),
            source_root: target_root.clone(),
            workspace_root: target_root,
        });
    }

    if mappings.len() != allowed_ids.len() {
        return Err("执行范围包含失效或未授权知识库。".to_owned());
    }
    Ok(mappings)
}

/** 将只读基线复制为可写工作副本，避免执行后丢失原始 hash 和正文。 */
fn copy_workspace_tree(source_root: &Path, target_root: &Path) -> Result<(), String> {
    fs::create_dir_all(target_root).map_err(|error| format!("无法创建 Skill 工作副本：{error}"))?;
    for entry in WalkDir::new(source_root).follow_links(false) {
        let entry = entry.map_err(|error| format!("无法复制 Skill 工作副本：{error}"))?;
        let relative = entry
            .path()
            .strip_prefix(source_root)
            .map_err(|_| "无法解析 Skill 工作副本路径。".to_owned())?;
        let target = target_root.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .map_err(|error| format!("无法创建 Skill 工作副本目录：{error}"))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("无法创建 Skill 工作副本父目录：{error}"))?;
            }
            fs::copy(entry.path(), target)
                .map_err(|error| format!("无法复制 Skill 工作副本文件：{error}"))?;
        }
    }
    Ok(())
}

/** 在 Windows AppContainer 中执行入口脚本；其他平台失败关闭。 */
#[cfg(windows)]
fn run_in_windows_sandbox(
    request: &SkillExecutionRequest,
    manifest: &crate::domain::SkillRuntimeManifest,
    skill_dir: &Path,
    runtime_path: &Path,
    workspace_root: &Path,
    limits: &crate::domain::AgentResourceLimits,
) -> Result<(), String> {
    use sandboxrs_windows::{BackendPreference, Sandbox};

    let runtime_parent = runtime_path
        .parent()
        .ok_or_else(|| "无法解析运行时目录。".to_owned())?;
    let sandbox = Sandbox::builder(workspace_root)
        .read_only(skill_dir)
        .read_only(runtime_parent)
        .timeout(Duration::from_secs(limits.timeout_seconds))
        .max_memory(limits.max_memory_mb.saturating_mul(1024 * 1024))
        .max_processes(limits.max_processes)
        .preferred_backend(BackendPreference::Auto)
        .identity("orange-skill")
        .build()
        .map_err(|error| format!("Windows 强沙箱不可用，已拒绝执行：{error}"))?;
    let entry_path = skill_dir.join(&manifest.entry);
    let mut command = sandbox.command(runtime_path);
    command.env_clear();
    command.current_dir(workspace_root);
    command.env("ORANGE_WORKSPACE", workspace_root);
    command.env("ORANGE_EXECUTION_ID", &request.id);

    match manifest.runtime.as_str() {
        "powershell" => {
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]);
            command.arg(&entry_path);
        }
        "executable" => {}
        _ => {
            command.arg(&entry_path);
        }
    }
    command.args(&manifest.args);
    command.args(&request.args);
    let output = command
        .output()
        .map_err(|error| format!("Skill 沙箱执行失败：{error}"))?;
    if !output.status.success() {
        let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr));
        return Err(format!("Skill 执行返回失败状态：{stderr}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_in_windows_sandbox(
    _request: &SkillExecutionRequest,
    _manifest: &crate::domain::SkillRuntimeManifest,
    _skill_dir: &Path,
    _runtime_path: &Path,
    _workspace_root: &Path,
    _limits: &crate::domain::AgentResourceLimits,
) -> Result<(), String> {
    Err("可执行 Skill 首版仅支持 Windows 11。".to_owned())
}

/** 比较隔离副本和真实知识库，只生成文本改动与新增二进制产物。 */
fn build_change_set(
    request: &SkillExecutionRequest,
    skill_id: &str,
    _workspace_root: &Path,
    mappings: &[WorkspaceRootMapping],
    max_artifact_mb: u64,
) -> Result<ProposedChangeSet, String> {
    let mut operations = Vec::new();
    let mut changed_bytes = 0_u64;

    for mapping in mappings {
        let source_files = collect_files(&mapping.source_root)?;
        let workspace_files = collect_files(&mapping.workspace_root)?;
        let all_paths = source_files
            .keys()
            .chain(workspace_files.keys())
            .cloned()
            .collect::<HashSet<_>>();

        for relative in all_paths {
            let source = source_files.get(&relative);
            let staged = workspace_files.get(&relative);
            if source.is_some()
                && staged.is_some()
                && file_hash(source.unwrap())? == file_hash(staged.unwrap())?
            {
                continue;
            }
            let binary = !is_editable_text_path(&relative);
            if binary && source.is_some() {
                return Err(format!(
                    "Skill 尝试修改或删除既有二进制文件 {}，已拒绝整次执行结果。",
                    portable_path(&relative)
                ));
            }
            let operation = match (source, staged) {
                (Some(_), None) => "delete",
                (None, Some(_)) => "create",
                (Some(_), Some(_)) => "modify",
                (None, None) => continue,
            };
            let byte_size = staged
                .and_then(|path| fs::metadata(path).ok())
                .map(|metadata| metadata.len() as usize)
                .unwrap_or_default();
            changed_bytes = changed_bytes.saturating_add(byte_size as u64);
            let original = source
                .filter(|_| !binary)
                .map(|path| {
                    fs::read_to_string(path).map_err(|error| format!("无法读取原始文本：{error}"))
                })
                .transpose()?;
            let next = staged
                .filter(|_| !binary)
                .map(|path| {
                    fs::read_to_string(path)
                        .map_err(|error| format!("Skill 输出不是有效 UTF-8 文本：{error}"))
                })
                .transpose()?;
            let original_hash = original
                .as_deref()
                .map(storage::hash_content)
                .unwrap_or_default();
            let staged_path = binary
                .then(|| staged.map(|path| path.to_string_lossy().to_string()))
                .flatten();

            operations.push(ProposedFileOperation {
                id: stable_operation_id(&mapping.knowledge_base_id, operation, &relative),
                knowledge_base_id: mapping.knowledge_base_id.clone(),
                operation: operation.to_owned(),
                source_path: (operation == "delete").then(|| portable_path(&relative)),
                target_path: portable_path(&relative),
                file_type: file_type_for_path(&relative),
                original_hash,
                original,
                next,
                selected: true,
                binary,
                byte_size,
                staged_path,
            });
        }
    }

    if changed_bytes > max_artifact_mb.saturating_mul(1024 * 1024) {
        return Err("Skill 产物超过单次执行上限，已拒绝生成变更集。".to_owned());
    }
    operations.sort_by(|left, right| {
        left.knowledge_base_id
            .cmp(&right.knowledge_base_id)
            .then_with(|| left.target_path.cmp(&right.target_path))
    });
    let summary = if operations.is_empty() {
        "Skill 执行完成，没有文件变化。".to_owned()
    } else {
        format!(
            "Skill 执行完成，生成 {} 项待确认文件变更。",
            operations.len()
        )
    };

    Ok(ProposedChangeSet {
        id: storage::create_id("change-set"),
        execution_id: request.id.clone(),
        skill_id: skill_id.to_owned(),
        status: "pending".to_owned(),
        summary,
        operations,
        warnings: Vec::new(),
        created_at: storage::format_local_datetime(),
    })
}

/** 操作 ID 由知识库、动作和相对路径派生，便于审批后从隔离基线安全重建。 */
fn stable_operation_id(knowledge_base_id: &str, operation: &str, relative: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(knowledge_base_id.as_bytes());
    hasher.update([0]);
    hasher.update(operation.as_bytes());
    hasher.update([0]);
    hasher.update(portable_path(relative).as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("operation-{}", &digest[..24])
}

/** 收集普通文件并拒绝 Skill 新建的符号链接，避免应用阶段解析到隔离区外。 */
fn collect_files(root: &Path) -> Result<HashMap<PathBuf, PathBuf>, String> {
    let mut files = HashMap::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| format!("无法读取 Skill 输出：{error}"))?;
        if entry.file_type().is_symlink() {
            return Err("Skill 输出包含符号链接，已拒绝整个变更集。".to_owned());
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| "无法解析 Skill 输出相对路径。".to_owned())?
            .to_path_buf();
        if should_skip_relative_path(&relative) || !is_supported_workspace_file(entry.path()) {
            continue;
        }
        files.insert(relative, entry.path().to_path_buf());
    }
    Ok(files)
}

fn should_skip_relative_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => matches!(
            name.to_string_lossy().as_ref(),
            ".git" | ".hg" | ".svn" | "node_modules" | "target" | "dist" | "build"
        ),
        Component::ParentDir | Component::RootDir | Component::Prefix(_) => true,
        Component::CurDir => false,
    })
}

fn is_supported_workspace_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "txt" | "docx" | "pdf" | "png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

fn is_editable_text_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "txt")
    )
}

fn file_type_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => "markdown",
        Some("txt") => "txt",
        Some("docx") => "docx",
        Some("pdf") => "pdf",
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => "image",
        _ => "binary",
    }
    .to_owned()
}

fn file_hash(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取文件用于比较：{error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn portable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn truncate_output(value: &str) -> String {
    value.chars().take(MAX_EXECUTION_OUTPUT_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_identifier_rejects_path_components() {
        assert!(validate_storage_identifier("execution-123", "执行 ID").is_ok());
        assert!(validate_storage_identifier("../escape", "执行 ID").is_err());
        assert!(validate_storage_identifier("a/b", "执行 ID").is_err());
    }

    #[test]
    fn operation_id_is_stable_for_same_target() {
        let first = stable_operation_id("kb-a", "modify", Path::new("Notes/demo.md"));
        let second = stable_operation_id("kb-a", "modify", Path::new("Notes/demo.md"));
        let other = stable_operation_id("kb-a", "delete", Path::new("Notes/demo.md"));

        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    /** apply_file_operation 的 create_folder 分支应递归创建目录且幂等。 */
    #[test]
    fn apply_file_operation_creates_folder_idempotently() {
        let root = tempfile::tempdir().expect("create temp root");
        let target = root.path().join("Notes").join("新目录");
        let operation = ProposedFileOperation {
            id: "op-test".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            operation: "create_folder".to_owned(),
            source_path: None,
            target_path: "Notes/新目录".to_owned(),
            file_type: "folder".to_owned(),
            original_hash: String::new(),
            original: None,
            next: None,
            selected: true,
            binary: false,
            byte_size: 0,
            staged_path: None,
        };

        apply_file_operation(&operation, &target).expect("first create_folder succeeds");
        assert!(target.is_dir());

        // 重复调用应幂等成功，不报错。
        apply_file_operation(&operation, &target).expect("second create_folder is idempotent");
        assert!(target.is_dir());
    }

    /** resolve_target_without_creation 必须拒绝路径穿越并放行合法相对路径。 */
    #[test]
    fn resolve_target_rejects_traversal_allows_relative() {
        let root = tempfile::tempdir().expect("create temp root");
        let kb_root = root.path().join("kb");
        fs::create_dir_all(&kb_root).expect("create kb root");

        let ok = resolve_target_without_creation(&kb_root, "Notes/子目录");
        assert!(ok.is_ok());

        let escaped = resolve_target_without_creation(&kb_root, "../escape");
        assert!(escaped.is_err());
    }

    /** can_auto_apply_agent_change_set 只在 autonomous+已开关且有 agent-direct 待确认变更集时放行。 */
    #[test]
    fn can_auto_apply_agent_change_set_gate() {
        use crate::domain::{
            AgentSecuritySettings, AgentSession, ImSessionIdentity, ProposedChangeSet,
        };

        fn session_with(
            level: &str,
            im: bool,
            change_set: Option<ProposedChangeSet>,
        ) -> AgentSession {
            AgentSession {
                id: "s".to_owned(),
                title: "t".to_owned(),
                im_identity: im.then(|| ImSessionIdentity {
                    provider_id: "feishu".to_owned(),
                    conversation_kind: "direct".to_owned(),
                    channel_hash: "x".to_owned(),
                    initial_message_preview: "m".to_owned(),
                    last_message_preview: "m".to_owned(),
                }),
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: None,
                pinned_note_ids: Vec::new(),
                messages: Vec::new(),
                pending_change: None,
                pending_change_set: change_set,
                pending_execution: None,
                security_level: level.to_owned(),
                context_summary: None,
                created_at: "t".to_owned(),
                updated_at: "t".to_owned(),
                deleted_at: None,
                model_provider_id: None,
                model_id: None,
            }
        }

        fn agent_change_set(status: &str) -> ProposedChangeSet {
            ProposedChangeSet {
                id: "cs".to_owned(),
                execution_id: AGENT_DIRECT_EXECUTION_ID.to_owned(),
                skill_id: "agent".to_owned(),
                status: status.to_owned(),
                summary: "test".to_owned(),
                operations: Vec::new(),
                warnings: Vec::new(),
                created_at: "t".to_owned(),
            }
        }

        let mut settings = AgentSecuritySettings::default();
        settings.autonomous_mode_enabled = true;

        let allowed = session_with("autonomous", false, Some(agent_change_set("pending")));
        assert!(can_auto_apply_agent_change_set(&allowed, &settings));

        // basic / advanced 不放行。
        assert!(!can_auto_apply_agent_change_set(
            &session_with("advanced", false, Some(agent_change_set("pending"))),
            &settings
        ));
        assert!(!can_auto_apply_agent_change_set(
            &session_with("basic", false, Some(agent_change_set("pending"))),
            &settings
        ));

        // IM 会话不放行。
        assert!(!can_auto_apply_agent_change_set(
            &session_with("autonomous", true, Some(agent_change_set("pending"))),
            &settings
        ));

        // autonomous_mode_enabled 关闭时不放行。
        let mut disabled = settings.clone();
        disabled.autonomous_mode_enabled = false;
        assert!(!can_auto_apply_agent_change_set(
            &session_with("autonomous", false, Some(agent_change_set("pending"))),
            &disabled
        ));

        // 非 agent-direct 变更集（Skill 路径）不放行。
        let skill_change_set = ProposedChangeSet {
            execution_id: "execution-123".to_owned(),
            ..agent_change_set("pending")
        };
        assert!(!can_auto_apply_agent_change_set(
            &session_with("autonomous", false, Some(skill_change_set)),
            &settings
        ));

        // 非 pending 状态不放行。
        assert!(!can_auto_apply_agent_change_set(
            &session_with("autonomous", false, Some(agent_change_set("applied"))),
            &settings
        ));
    }

    /** 本机安全探针：AppContainer 可写 workspace，但不能写同级未授权目录。 */
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a live Windows AppContainer backend"]
    fn windows_appcontainer_denies_write_outside_workspace() {
        use sandboxrs_windows::{BackendPreference, Sandbox};

        let root = tempfile::tempdir().expect("create sandbox probe root");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside.txt");
        fs::create_dir_all(&workspace).expect("create workspace");
        let command_text = format!(
            "echo inside>\"{}\" & echo outside>\"{}\"",
            workspace.join("inside.txt").display(),
            outside.display()
        );
        let sandbox = Sandbox::builder(&workspace)
            .preferred_backend(BackendPreference::Auto)
            .timeout(Duration::from_secs(10))
            .max_memory(128 * 1024 * 1024)
            .max_processes(2)
            .identity("orange-probe")
            .build()
            .expect("build AppContainer sandbox");
        let _ = sandbox
            .command(r"C:\Windows\System32\cmd.exe")
            .args(["/D", "/C", &command_text])
            .output();

        assert!(workspace.join("inside.txt").exists());
        assert!(!outside.exists());
    }
}
