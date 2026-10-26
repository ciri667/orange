//! 结构化文件变更：可信路径、完整目录清单、逐项持久化和严格磁盘冲突检查。
use crate::{domain::*, storage, agent_tools::{AgentToolContext, ToolExecutionResult}};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::{Path, PathBuf}};
use tauri::AppHandle;

fn error(e: impl std::fmt::Display) -> String { e.to_string() }

/** 旧单文件草稿转为可信变更集；读取失败或版本变化时保留记录并要求重新提出变更。 */
pub(crate) fn migrate_legacy_pending(session: &mut AgentSession, knowledge_bases: &[KnowledgeBase]) {
    if session.im_identity.is_some() { return; }
    let Some(change) = session.pending_change.as_ref().filter(|c| c.status == "pending").cloned() else { return; };
    if session.pending_change_set.as_ref().is_some_and(|s| matches!(s.status.as_str(), "pending" | "partial")) { return; }
    let body = if change.r#type == "create" { Ok((String::new(), change.next.clone())) } else {
        crate::fs_guard::resolve_persisted_operation_target(knowledge_bases, &session.knowledge_base_ids,
            &session.security_level, &change.knowledge_base_id, &change.target_path).and_then(|path| {
            let current = fs::read_to_string(path).map_err(error)?;
            let hash = storage::hash_content(&current);
            let next = crate::agent_writes::apply_rewrite_change(&current, &hash, &hash, &change)?;
            Ok((current,next))
        })
    };
    let (original,next,failure) = match body {
        Ok((original,next)) => (Some(original),Some(next),None),
        Err(e) => (Some(change.original.clone()),None,Some(format!("旧草稿不能安全恢复：{e}；请重新读取并提出变更。"))),
    };
    let warnings = failure.iter().cloned().collect();
    session.pending_change_set = Some(ProposedChangeSet {
        id: format!("legacy-{}",change.id), execution_id: AGENT_DIRECT_EXECUTION_ID.into(), skill_id: AGENT_DIRECT_SOURCE.into(),
        status: "pending".into(), summary: change.title.clone(), created_at: session.updated_at.clone(), warnings,
        operations: vec![ProposedFileOperation {
            id: change.id.clone(), knowledge_base_id: change.knowledge_base_id.clone(),
            operation: if change.r#type == "create" {"create"} else {"modify"}.into(), source_path: None,
            target_path: change.target_path.clone(), file_type: change.file_type.clone().unwrap_or_else(||"markdown".into()),
            original_hash: change.original_hash.clone(), byte_size: next.as_ref().map_or(0,String::len), original, next,
            selected: true, binary: false, staged_path: None, status: if failure.is_some() {"failed"} else {"pending"}.into(),
            error: failure, affected_files: 1,
        }],
    });
    // 保留旧评论与 diff 元数据以供历史查看，但禁止两个审批入口重复应用。
    if let Some(old) = session.pending_change.as_mut() { old.status = "migrated".into(); }
}

/** 拒绝任意路径分段中的链接，防止审批期间用链接替换真实目录。 */
pub fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("路径包含链接，已拒绝。".into()),
            Ok(meta) => {
                // Windows 目录联接点同样能跨出授权根，不能只检查符号链接。
                #[cfg(windows)] {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 { return Err("路径包含重解析点，已拒绝。".into()); }
                }
                let _ = meta;
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(error(e)),
        }
    }
    Ok(())
}

/** 不使用索引忽略规则；哈希覆盖隐藏文件、目录名和全部文件字节。 */
pub fn fingerprint(path: &Path) -> Result<(String, usize, usize), String> {
    reject_links(path)?;
    let mut entries = walkdir::WalkDir::new(path).follow_links(false).into_iter()
        .collect::<Result<Vec<_>, _>>().map_err(error)?;
    entries.sort_by(|a,b| a.path().cmp(b.path()));
    let mut digest = Sha256::new(); let mut count = 0; let mut bytes = 0;
    for entry in entries {
        reject_links(entry.path())?;
        let relative = entry.path().strip_prefix(path).map_err(error)?.to_string_lossy();
        digest.update(relative.len().to_le_bytes()); digest.update(relative.as_bytes());
        let meta = fs::metadata(entry.path()).map_err(error)?;
        if meta.is_dir() { digest.update(b"dir"); continue; }
        if !meta.is_file() { return Err("不支持设备、管道或其它特殊文件。".into()); }
        digest.update(b"file"); digest.update(meta.len().to_le_bytes());
        let mut file = fs::File::open(entry.path()).map_err(error)?;
        let mut buffer = [0; 65536];
        loop { let n = file.read(&mut buffer).map_err(error)?; if n == 0 { break; } digest.update(&buffer[..n]); }
        count += 1; bytes += meta.len() as usize;
    }
    Ok((format!("{:x}", digest.finalize()), count, bytes))
}

fn resolve(snapshot: &WorkspaceSnapshot, index: usize, kb: Option<&str>, path: &str, exists: bool) -> Result<crate::fs_guard::ResolvedFsTarget, String> {
    let session = &snapshot.sessions[index];
    let active = session.knowledge_base_ids.first().ok_or("当前会话没有激活知识库")?;
    let target = crate::fs_guard::resolve_agent_fs_target(&snapshot.knowledge_bases, &session.knowledge_base_ids,
        active, &session.security_level, kb, path, exists)?;
    reject_links(&target.absolute_path)?;
    Ok(target)
}

/** 源和目标均进入冲突比较，拒绝重复目标、重叠目录及兄弟任务覆盖。 */
pub fn append(snapshot: &mut WorkspaceSnapshot, index: usize, operation: ProposedFileOperation) -> Result<(), String> {
    let target = resolve(snapshot, index, Some(&operation.knowledge_base_id), &operation.target_path, false)?.absolute_path;
    let source = operation.source_path.as_ref().map(PathBuf::from);
    for session in &snapshot.sessions {
        if let Some(set) = session.pending_change_set.as_ref().filter(|s| matches!(s.status.as_str(), "pending" | "partial")) {
            for existing in set.operations.iter().filter(|o| o.status != "completed") {
                let existing_target = crate::fs_guard::resolve_persisted_operation_target(&snapshot.knowledge_bases,
                    &session.knowledge_base_ids, &session.security_level, &existing.knowledge_base_id, &existing.target_path)?;
                let mut old = vec![existing_target]; if let Some(p) = &existing.source_path { old.push(p.into()); }
                let mut new = vec![target.clone()]; if let Some(p) = &source { new.push(p.clone()); }
                if old.iter().any(|a| new.iter().any(|b| a.starts_with(b) || b.starts_with(a))) {
                    return Err("该路径与已有待审操作重叠；请先处理父级或兄弟任务的变更。".into());
                }
            }
        }
    }
    let session = &mut snapshot.sessions[index];
    let set = session.pending_change_set.get_or_insert_with(|| ProposedChangeSet {
        id: storage::create_id("change-set"), execution_id: AGENT_DIRECT_EXECUTION_ID.into(), skill_id: AGENT_DIRECT_SOURCE.into(),
        status: "pending".into(), summary: "文件变更".into(), operations: vec![], warnings: vec![], created_at: storage::format_local_datetime(),
    });
    if !matches!(set.status.as_str(), "pending" | "partial") {
        *set = ProposedChangeSet { id: storage::create_id("change-set"), execution_id: AGENT_DIRECT_EXECUTION_ID.into(), skill_id: AGENT_DIRECT_SOURCE.into(),
            status: "pending".into(), summary: "文件变更".into(), operations: vec![], warnings: vec![], created_at: storage::format_local_datetime() };
    }
    if matches!(operation.operation.as_str(), "move" | "rename") {
        set.warnings.push("移动可能影响 Markdown 相对链接；正文链接不会自动重写。".into());
    }
    set.operations.push(operation); set.status = "pending".into();
    session.updated_at = storage::format_local_datetime();
    Ok(())
}

/** 通用 edit/write，文本编辑仅接受 Markdown/TXT，管理操作可覆盖二进制和完整目录。 */
pub fn propose(context: &mut AgentToolContext<'_>, name: &str, args: &Value) -> ToolExecutionResult {
    if let Err(e) = crate::permissions::ensure(&context.snapshot.sessions[context.session_index], crate::permissions::tool_operation(name,args)) {
        return ToolExecutionResult::failed(&e);
    }
    match prepare(context.snapshot, context.session_index, name, args).and_then(|op| {
        let receipt = json!({"operation": op.operation, "sourcePath": op.source_path, "targetPath": op.target_path, "affectedFiles": op.affected_files});
        append(context.snapshot, context.session_index, op)?;
        Ok(receipt)
    }) {
        Ok(receipt) => ToolExecutionResult { success: true, summary: "已生成待审文件变更。".into(), payload: json!({"changeSet": context.snapshot.sessions[context.session_index].pending_change_set, "receipt": receipt}), citations: vec![], audit_fragment: None },
        Err(e) => ToolExecutionResult::failed(&e),
    }
}

fn prepare(snapshot: &WorkspaceSnapshot, index: usize, name: &str, args: &Value) -> Result<ProposedFileOperation, String> {
    let kb = args["knowledgeBaseId"].as_str();
    let operation = if name == "write" { if crate::agent_tools::write_kind(args) == "folder" { "create_folder" } else { "create" } }
        else { args["operation"].as_str().unwrap_or("replace") };
    let management = matches!(operation, "move" | "rename" | "delete");
    let path = args["path"].as_str().or(args["targetPath"].as_str());
    let (kb_owned, path_owned) = if let Some(id) = args["fileId"].as_str().or(args["noteId"].as_str()) {
        if let Some(note) = snapshot.notes.iter().find(|f| f.id == id) { (note.knowledge_base_id.clone(), note.path.clone()) }
        else if let Some(doc) = snapshot.documents.iter().find(|f| f.id == id) { (doc.knowledge_base_id.clone(), doc.path.clone()) }
        else { return Err("目标文件不存在。".into()); }
    } else { (kb.unwrap_or("").to_owned(), path.ok_or("缺少 path 或 targetPath")?.to_owned()) };
    let target = resolve(snapshot, index, (!kb_owned.is_empty()).then_some(kb_owned.as_str()), &path_owned, name == "edit")?;
    let mut result = ProposedFileOperation { id: storage::create_id("file-op"), knowledge_base_id: target.knowledge_base_id.clone(),
        operation: if management || name == "write" { operation } else { "modify" }.into(), source_path: None,
        target_path: target.stored_path.clone(), file_type: "folder".into(), original_hash: String::new(),
        original: None, next: None, selected: true, binary: false, byte_size: 0, staged_path: None,
        status: "pending".into(), error: None, affected_files: 0 };
    if management {
        reject_root(snapshot, &target.absolute_path)?;
        let (hash, count, bytes) = fingerprint(&target.absolute_path)?;
        result.original_hash = hash; result.affected_files = count; result.byte_size = bytes;
        result.source_path = Some(target.absolute_path.to_string_lossy().into_owned());
        if !target.absolute_path.is_dir() { result.file_type = extension(&target.absolute_path); result.binary = !is_text(&target.absolute_path); }
        if operation != "delete" {
            let destination = args["destination"].as_str().or(args["destinationPath"].as_str()).ok_or("移动/重命名缺少 destination")?;
            let next = resolve(snapshot, index, args["destinationKnowledgeBaseId"].as_str().or(kb), destination, false)?;
            if next.absolute_path.exists() { return Err("目标已存在，禁止覆盖。".into()); }
            if next.absolute_path.starts_with(&target.absolute_path) { return Err("不能将目录移入自身。".into()); }
            same_volume(&target.absolute_path, next.absolute_path.parent().ok_or("目标缺少父目录")?)?;
            result.knowledge_base_id = next.knowledge_base_id; result.target_path = next.stored_path;
        }
    } else if name == "write" {
        if target.absolute_path.exists() { return Err("创建目标已存在，禁止覆盖。".into()); }
        if operation != "create_folder" {
            if !is_text(&target.absolute_path) { return Err("只能创建 Markdown/TXT 正文。".into()); }
            let text = args["content"].as_str().or(args["next"].as_str()).ok_or("缺少 content")?;
            result.file_type = extension(&target.absolute_path); result.next = Some(text.into()); result.byte_size = text.len(); result.affected_files = 1;
        }
    } else {
        if !is_text(&target.absolute_path) { return Err("仅 Markdown/TXT 支持正文编辑。".into()); }
        let original = fs::read_to_string(&target.absolute_path).map_err(error)?;
        let prepared = crate::agent_tools::execute::prepare_rewrite_content(&original, operation, args, extension(&target.absolute_path) == "markdown")?;
        let shape = crate::text_edit::TextFileShape::detect(&original);
        let normalized = shape.normalize(&original);
        let next = if matches!(operation, "append" | "multi_replace") { prepared.next }
            else if let Some(n) = prepared.occurrence { crate::text_edit::replace_occurrence(&normalized, &prepared.original, &prepared.next, n).map_err(|_| "替换定位失败")? }
            else { crate::text_edit::replace_unique(&normalized, &prepared.original, &prepared.next).map_err(|_| "替换定位失败")? };
        result.original_hash = storage::hash_content(&original); result.original = Some(original); result.next = Some(shape.restore(&next));
        result.byte_size = result.next.as_ref().map_or(0, String::len);
        result.file_type = extension(&target.absolute_path); result.affected_files = 1;
    }
    Ok(result)
}

fn is_text(path: &Path) -> bool { matches!(path.extension().and_then(|e|e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("md" | "markdown" | "txt")) }
fn extension(path: &Path) -> String { if storage::is_markdown_file(path) { "markdown".into() } else { path.extension().and_then(|e| e.to_str()).unwrap_or("binary").to_ascii_lowercase() } }
fn reject_root(snapshot: &WorkspaceSnapshot, path: &Path) -> Result<(), String> {
    for kb in &snapshot.knowledge_bases {
        if fs::canonicalize(&kb.path).is_ok_and(|root| root.starts_with(path)) {
            return Err("禁止删除、移动或重命名知识库根目录及包含它的目录。".into());
        }
    }
    Ok(())
}

/** 同卷 rename；绝不退化为复制后删除。 */
fn same_volume(source: &Path, target_parent: &Path) -> Result<(), String> {
    let mut parent = target_parent; while !parent.exists() { parent = parent.parent().ok_or("目标没有有效父目录")?; }
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(source).map_err(error)?.dev() != fs::metadata(parent).map_err(error)?.dev() { return Err("本轮不支持跨卷移动；源文件保留。".into()); }
    }
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        fn volume(path: &Path) -> Result<Vec<u16>, String> {
            let input = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>(); let mut out = vec![0; 32768];
            if unsafe { windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW(input.as_ptr(), out.as_mut_ptr(), out.len() as u32) } == 0 { return Err(error(std::io::Error::last_os_error())); }
            out.truncate(out.iter().position(|c| *c == 0).unwrap_or(out.len())); Ok(out)
        }
        if volume(source)? != volume(parent)? { return Err("本轮不支持跨卷移动；源文件保留。".into()); }
    }
    Ok(())
}

fn validate(snapshot: &WorkspaceSnapshot, index: usize, op: &ProposedFileOperation) -> Result<(PathBuf, Option<PathBuf>), String> {
    let target = resolve(snapshot, index, Some(&op.knowledge_base_id), &op.target_path, false)?.absolute_path;
    let source = op.source_path.as_ref().map(|path| resolve(snapshot,index,None,path,true).map(|t|t.absolute_path)).transpose()?;
    match op.operation.as_str() {
        "create" | "create_folder" => if target.exists() { return Err("创建目标已经存在。".into()); },
        "modify" => {
            let current = fs::read_to_string(&target).map_err(error)?;
            if storage::hash_content(&current) != op.original_hash { return Err(crate::agent_writes::STALE_FILE_WRITE_ERROR.into()); }
        },
        "move" | "rename" | "delete" => {
            let source = source.as_ref().ok_or("缺少可信源路径")?; reject_root(snapshot,source)?;
            if fingerprint(source)?.0 != op.original_hash { return Err("源文件或目录完整清单已变化，请重新生成预览。".into()); }
            if op.operation != "delete" {
                if target.exists() || target.starts_with(source) { return Err("目标已存在或位于源目录内。".into()); }
                same_volume(source,target.parent().ok_or("目标缺少父目录")?)?;
            }
        },
        _ => return Err("未知文件操作。".into()),
    }
    Ok((target,source))
}

/** 只接受 ID 与选择；从后端重载路径、正文、hash 和已完成状态。 */
pub fn apply(app: &AppHandle, session_id: &str, set_id: &str, selected_ids: &[String]) -> Result<WorkspaceSnapshot, String> {
    // 锁内重载已完成状态，重复点击或并发确认不能重放已执行操作。
    crate::agent_writes::with_write_paths(&[], || apply_locked(app,session_id,set_id,selected_ids))
}

fn apply_locked(app: &AppHandle, session_id: &str, set_id: &str, selected_ids: &[String]) -> Result<WorkspaceSnapshot, String> {
    let mut snapshot = storage::load_workspace_snapshot(app)?;
    let index = snapshot.sessions.iter().position(|s| s.id == session_id).ok_or("会话不存在")?;
    snapshot.active_session_id = session_id.into();
    crate::permissions::ensure(&snapshot.sessions[index], crate::permissions::Operation::Apply)?;
    if snapshot.sessions[index].im_identity.is_some() { return Err("IM 仅允许原身份绑定的单文件确认流程。".into()); }
    let mut set = snapshot.sessions[index].pending_change_set.clone().filter(|s| s.id == set_id && matches!(s.status.as_str(), "pending" | "partial"))
        .ok_or("变更集已过期或已处理")?;
    let selected = set.operations.iter().enumerate().filter(|(_,o)| o.status != "completed" && selected_ids.contains(&o.id)).map(|(i,_)| i).collect::<Vec<_>>();
    if selected.is_empty() { return Err("没有可应用的操作。".into()); }
    for i in &selected { validate(&snapshot,index,&set.operations[*i])?; }
    (|| {
        crate::operation_approval::check_current(app,&snapshot.sessions[index])?;
        for i in &selected { validate(&snapshot,index,&set.operations[*i])?; }
        for i in selected {
            let op = &set.operations[i];
            let result = validate(&snapshot,index,op).and_then(|(target,source)| {
                crate::operation_approval::check_current(app,&snapshot.sessions[index])?;
                if op.operation == "modify" { capture_history(app,&snapshot,session_id,&target)?; }
                match op.operation.as_str() {
                    "create_folder" => fs::create_dir(&target).map_err(error),
                    "create" | "modify" => {
                        if let Some(parent) = target.parent() { if !parent.is_dir() { return Err("目标父目录不存在；请先独立创建目录。".into()); } }
                        let content=op.next.as_deref().ok_or("缺少可信正文")?;
                        if op.operation == "create" { atomic_create(&target,content) }
                        else { storage::atomic_write_text_document(&target,content) }
                    },
                    "delete" => trash::delete(source.as_ref().ok_or("缺少源路径")?).map_err(error),
                    "move" | "rename" => {
                        let source = source.as_ref().ok_or("缺少源路径")?;
                        rename_without_replace(source,&target)?;
                        // 文件已经移动，引用更新失败不能把它重新标为可重试的未执行操作。
                        if let Err(e) = update_references(app,&mut snapshot,source,&target) {
                            set.warnings.push(format!("文件已移动，但内部引用更新失败：{e}；请刷新后检查历史。"));
                        }
                        Ok(())
                    },
                    _ => Err("未知操作".into()),
                }
            });
            match result {
                Ok(()) => { set.operations[i].status = "completed".into(); set.operations[i].error = None; },
                Err(e) => { set.operations[i].status = "failed".into(); set.operations[i].error = Some(e); set.status = "partial".into(); },
            }
            snapshot.sessions[index].pending_change_set = Some(set.clone());
            storage::save_snapshot_session(app,&snapshot,session_id)?;
            if set.operations[i].status == "failed" { break; }
        }
        if set.operations.iter().all(|o| o.status == "completed") { set.status = "applied".into(); }
        else if set.operations.iter().any(|o| o.status == "completed") { set.status = "partial".into(); }
        snapshot.sessions[index].pending_change_set = Some(set);
        refresh(app,&mut snapshot)?;
        storage::save_snapshot_session(app,&snapshot,session_id)?;
        Ok(snapshot)
    })()
}

/** 新建文本以不覆盖方式持久化临时文件，避免确认后出现的同名文件被替换。 */
fn atomic_create(path: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or("目标没有父目录")?).map_err(error)?;
    temporary.write_all(content.as_bytes()).map_err(error)?;
    temporary.as_file().sync_all().map_err(error)?;
    temporary.persist_noclobber(path).map_err(error)?;
    Ok(())
}

/** 同卷、禁止覆盖的系统原语；不提供复制后删除的退路。 */
fn rename_without_replace(source: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        let from = source.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let to = target.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
        if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(),to.as_ptr(),0) } == 0 {
            return Err(error(std::io::Error::last_os_error()));
        }
    }
    #[cfg(unix)] {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(error)?;
        let to = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(error)?;
        #[cfg(target_os="linux")]
        let result = unsafe { libc::renameat2(libc::AT_FDCWD,from.as_ptr(),libc::AT_FDCWD,to.as_ptr(),libc::RENAME_NOREPLACE) };
        #[cfg(target_os="macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(),to.as_ptr(),libc::RENAME_EXCL) };
        if result != 0 { return Err(error(std::io::Error::last_os_error())); }
    }
    Ok(())
}

fn capture_history(app: &AppHandle, snapshot: &WorkspaceSnapshot, session_id: &str, path: &Path) -> Result<(), String> {
    // 复用已有单文件历史入口，由原子写入前保存当前磁盘版本。
    crate::agent_writes::capture_file_history(app,snapshot,session_id,path)
}

/** 重扫始终用磁盘事实；前端自己的未保存缓冲由编辑器继续保留并显示冲突。 */
pub fn refresh(app: &AppHandle, snapshot: &mut WorkspaceSnapshot) -> Result<(), String> {
    for previous in snapshot.knowledge_bases.clone() {
        let selection = KnowledgeBaseSelection { id: previous.id.clone(), name: previous.name.clone(), path: previous.path.clone(), note_count: previous.note_count };
        let (mut kb, folders, notes, documents) = storage::scan_supported_documents_directory(&selection)?;
        kb.is_default = previous.is_default; kb.semantic_index_enabled = previous.semantic_index_enabled;
        if let Some(current) = snapshot.knowledge_bases.iter_mut().find(|k|k.id==kb.id) { *current=kb; }
        snapshot.folders.retain(|f|f.knowledge_base_id!=previous.id); snapshot.folders.extend(folders);
        snapshot.notes.retain(|f|f.knowledge_base_id!=previous.id); snapshot.notes.extend(notes);
        snapshot.documents.retain(|f|f.knowledge_base_id!=previous.id); snapshot.documents.extend(documents);
    }
    storage::index_snapshot(app,snapshot)
}

fn update_references(app: &AppHandle, snapshot: &mut WorkspaceSnapshot, source: &Path, target: &Path) -> Result<(), String> {
    crate::storage::move_file_references(app,snapshot,source,target)
}

/** 仅在一次成功的 edit/write 后调用，旧 pending 不会随读取、切档或 task 启动被应用。 */
pub(crate) fn auto_apply_new(app: &AppHandle, snapshot: &mut WorkspaceSnapshot, index: usize, previous: &[String]) -> Result<Option<Vec<String>>,String> {
    if !crate::agent_writes::allows_autonomous_auto_apply(&snapshot.sessions[index],&AgentSecuritySettings::default()) { return Ok(None); }
    let session_id=snapshot.sessions[index].id.clone();
    let Some(set)=snapshot.sessions[index].pending_change_set.as_ref() else {return Ok(None)};
    let set_id=set.id.clone();
    let new_ids=set.operations.iter().filter(|op|op.status!="completed" && !previous.contains(&op.id)).map(|op|op.id.clone()).collect::<Vec<_>>();
    if new_ids.is_empty() { return Ok(None); }
    storage::save_snapshot_session(app,snapshot,&session_id)?;
    *snapshot=apply(app,&session_id,&set_id,&new_ids)?;
    Ok(Some(new_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace(root: &Path, level: &str) -> WorkspaceSnapshot {
        serde_json::from_value(json!({
            "knowledgeBases":[{"id":"kb-a","name":"测试","path":root,"description":"","status":"ready","noteCount":0,"documentCount":0,"updatedAt":"t","isDefault":true,"semanticIndexEnabled":false}],
            "folders":[],"notes":[],"documents":[],"sessions":[crate::permissions::test_session(level)],
            "activeKnowledgeBaseId":"kb-a","activeSessionId":"permission-test","activeNoteId":"","activeDocumentId":""
        })).unwrap()
    }
    #[test]
    fn external_read_paths_cannot_be_hidden_behind_an_in_scope_file_id() {
        let inside = tempfile::tempdir().unwrap(); let outside = tempfile::tempdir().unwrap();
        let mut snapshot = workspace(inside.path(), "full-confirm");
        fs::write(inside.path().join("note.md"), "inside").unwrap();
        fs::write(outside.path().join("external.md"), "outside").unwrap();
        let selection = KnowledgeBaseSelection { id:"kb-a".into(), name:"测试".into(),
            path:inside.path().to_string_lossy().into_owned(), note_count:0 };
        snapshot.notes = storage::scan_supported_documents_directory(&selection).unwrap().2;
        let id = &snapshot.notes[0].id;
        for name in ["read", "list", "search"] {
            let args = json!({"fileId":id, "path":outside.path().join("external.md")});
            assert_eq!(crate::agent_tools::external::decision(&snapshot,0,name,&args).unwrap(), crate::permissions::Decision::RequireApproval);
        }
        let args = json!({"fileId":id, "path":outside.path().join("external.md"), "original":"inside", "next":"next"});
        assert_eq!(crate::agent_tools::external::decision(&snapshot,0,"edit",&args).unwrap(), crate::permissions::Decision::Allow);
    }

    #[test]
    fn directory_fingerprint_includes_hidden_and_unindexed_changes() {
        let dir=tempfile::tempdir().unwrap(); fs::write(dir.path().join(".hidden"),"a").unwrap();
        let before=fingerprint(dir.path()).unwrap(); assert_eq!(before.1,1);
        fs::write(dir.path().join("ignored.bin"),[0u8,255]).unwrap();
        assert_ne!(before.0,fingerprint(dir.path()).unwrap().0);
    }

    #[test]
    fn text_change_uses_disk_preserves_bom_crlf_and_rejects_stale_hash() {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("note.md");
        fs::write(&path,"\u{feff}# 标题\r\n\r\n正文\r\n").unwrap();
        let snapshot=workspace(dir.path(),"workspace-write");
        let op=prepare(&snapshot,0,"edit",&json!({"path":"note.md","original":"正文","next":"新文"})).unwrap();
        assert_eq!(op.next.as_deref(),Some("\u{feff}# 标题\r\n\r\n新文\r\n"));
        validate(&snapshot,0,&op).unwrap();
        fs::write(&path,"另一个会话的版本").unwrap();
        assert!(validate(&snapshot,0,&op).unwrap_err().contains("更新"));
        fs::remove_file(path).unwrap();
        assert!(validate(&snapshot,0,&op).is_err());
    }

    #[test]
    fn directory_management_rechecks_complete_contents_and_protects_roots() {
        let dir=tempfile::tempdir().unwrap(); fs::create_dir(dir.path().join("folder")).unwrap();
        fs::write(dir.path().join("folder/image.bin"),[0,255]).unwrap();
        let snapshot=workspace(dir.path(),"workspace-write");
        let op=prepare(&snapshot,0,"edit",&json!({"path":"folder","operation":"delete"})).unwrap();
        assert_eq!(op.affected_files,1);
        fs::write(dir.path().join("folder/.hidden"),"later").unwrap();
        assert!(validate(&snapshot,0,&op).is_err());
        assert!(prepare(&snapshot,0,"edit",&json!({"path":".","operation":"delete"})).is_err());
        assert!(prepare(&snapshot,0,"edit",&json!({"path":"folder","operation":"move","destination":"folder/child"})).is_err());
    }

    #[test]
    fn writer_operations_accumulate_and_overlapping_sibling_paths_are_rejected() {
        let dir=tempfile::tempdir().unwrap(); let mut snapshot=workspace(dir.path(),"workspace-write");
        for name in ["a.md","b.txt"] {
            let op=prepare(&snapshot,0,"write",&json!({"targetPath":name,"content":"正文"})).unwrap();
            append(&mut snapshot,0,op).unwrap();
        }
        assert_eq!(snapshot.sessions[0].pending_change_set.as_ref().unwrap().operations.len(),2);
        let repeated=prepare(&snapshot,0,"write",&json!({"targetPath":"a.md","content":"覆盖"})).unwrap();
        assert!(append(&mut snapshot,0,repeated).is_err());
        let mut sibling=crate::permissions::test_session("workspace-write"); sibling.id="sibling".into(); snapshot.sessions.push(sibling);
        let overlap=prepare(&snapshot,1,"write",&json!({"targetPath":"a.md","content":"覆盖"})).unwrap();
        assert!(append(&mut snapshot,1,overlap).is_err());
    }

    #[test]
    fn create_and_move_never_overwrite_existing_targets() {
        let dir=tempfile::tempdir().unwrap(); let source=dir.path().join("source.bin"); let target=dir.path().join("target.bin");
        fs::write(&source,[1,2,3]).unwrap(); fs::write(&target,[4,5,6]).unwrap();
        assert!(rename_without_replace(&source,&target).is_err());
        assert_eq!(fs::read(&source).unwrap(),vec![1,2,3]); assert_eq!(fs::read(&target).unwrap(),vec![4,5,6]);
        assert!(atomic_create(&target,"overwrite").is_err());
        fs::remove_file(&target).unwrap(); rename_without_replace(&source,&target).unwrap();
        assert!(!source.exists()); assert_eq!(fs::read(target).unwrap(),vec![1,2,3]);
    }

    #[test]
    fn legacy_pending_migration_keeps_comments_and_never_applies_itself() {
        let dir=tempfile::tempdir().unwrap(); fs::write(dir.path().join("note.md"),"旧内容").unwrap();
        let mut snapshot=workspace(dir.path(),"workspace-write");
        snapshot.sessions[0].pending_change=Some(serde_json::from_value(json!({
            "id":"legacy-change","knowledgeBaseId":"kb-a","type":"rewrite","title":"旧草稿","targetPath":"note.md",
            "original":"旧","next":"新","originalHash":storage::hash_content("旧内容"),"status":"pending"
        })).unwrap());
        migrate_legacy_pending(&mut snapshot.sessions[0],&snapshot.knowledge_bases);
        let set=snapshot.sessions[0].pending_change_set.clone().unwrap();
        assert_eq!(set.operations[0].next.as_deref(),Some("新内容"));
        assert_eq!(fs::read_to_string(dir.path().join("note.md")).unwrap(),"旧内容");
        migrate_legacy_pending(&mut snapshot.sessions[0],&snapshot.knowledge_bases);
        assert_eq!(snapshot.sessions[0].pending_change_set.as_ref().unwrap().id,set.id);
        assert_eq!(snapshot.sessions[0].pending_change.as_ref().unwrap().status,"migrated");
    }
}
