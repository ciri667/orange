use super::*;

/** 移动后的内部 ID 与历史迁移；正文 Markdown 链接明确保持原样。 */
pub fn move_file_references(app: &AppHandle, snapshot: &mut WorkspaceSnapshot, source: &Path, target: &Path) -> Result<(), String> {
    let mut mappings = Vec::new();
    for note in &snapshot.notes {
        if let Some(kb) = snapshot.knowledge_bases.iter().find(|k|k.id==note.knowledge_base_id) {
            let old = fs::canonicalize(&kb.path).map_err(|e|e.to_string())?.join(&note.path);
            if let Ok(suffix)=old.strip_prefix(source) { mappings.push(("note",note.id.clone(),target.join(suffix),note.title.clone())); }
        }
    }
    for doc in &snapshot.documents {
        if let Some(kb) = snapshot.knowledge_bases.iter().find(|k|k.id==doc.knowledge_base_id) {
            let old = fs::canonicalize(&kb.path).map_err(|e|e.to_string())?.join(&doc.path);
            if let Ok(suffix)=old.strip_prefix(source) { mappings.push(("document",doc.id.clone(),target.join(suffix),doc.title.clone())); }
        }
    }
    for (kind, old_id, path, title) in mappings {
        let destination=snapshot.knowledge_bases.iter().find_map(|kb|fs::canonicalize(&kb.path).ok().and_then(|root|path.strip_prefix(root).ok()).map(|relative|(kb.id.clone(),relative.to_string_lossy().replace('\\',"/"))));
        let (new_kb, new_path) = destination.clone().unwrap_or_else(|| (crate::domain::EXTERNAL_FILESYSTEM_SCOPE_ID.into(),path.to_string_lossy().into_owned()));
        let next_id = if let Some((kb, relative)) = destination {
            let id=if kind=="note" {create_stable_note_id(&kb,&relative)} else {create_stable_document_id(&kb,&relative)};
            migrate_document_history_target(app,kind,&old_id,&id,&kb,&relative,&title)?;
            id
        } else {
            let id=if kind=="note" {create_stable_note_id(&new_kb,&new_path)} else {create_stable_document_id(&new_kb,&new_path)};
            migrate_document_history_target(app,kind,&old_id,&id,&new_kb,&new_path,&title)?;
            id
        };
        let name = snapshot.knowledge_bases.iter().find(|k| k.id == new_kb).map(|k|k.name.clone()).unwrap_or_else(||"工作区外文件".into());
        // 只改最新持久化记录中的文件引用，不能用移动前快照覆盖其它会话的新消息或权限。
        let mut connection=open_database(app)?;
        let _writer=lock_database_writer()?;
        let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        for session in &mut snapshot.sessions {
            rewrite_session_references(session,&old_id,&next_id,&new_kb,&new_path,&name);
            let payload: Option<String>=transaction.query_row("SELECT payload_json FROM agent_sessions WHERE id=?1",params![&session.id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if let Some(payload)=payload {
                let mut current: crate::domain::AgentSession=serde_json::from_str(&payload).map_err(|e|e.to_string())?;
                rewrite_session_references(&mut current,&old_id,&next_id,&new_kb,&new_path,&name);
                super::sessions::persist_session_in_transaction(&transaction,&current)?;
            }
        }
        transaction.commit().map_err(|e|e.to_string())?;
        if snapshot.active_note_id==old_id { snapshot.active_note_id=next_id.clone(); }
        if snapshot.active_document_id==old_id { snapshot.active_document_id=next_id; }
    }
    Ok(())
}

/** 聊天引用、@ 文件、会话选中文件同步到新的稳定 ID；Markdown 正文不改。 */
fn rewrite_session_references(session: &mut crate::domain::AgentSession, old: &str, next: &str, kb: &str, path: &str, name: &str) {
    if session.active_note_id.as_deref()==Some(old) { session.active_note_id=Some(next.into()); }
    for id in &mut session.pinned_note_ids { if id==old { *id=next.into(); } }
    for message in &mut session.messages {
        for id in &mut message.mentioned_file_ids { if id==old { *id=next.into(); } }
        if let Some(citations)=message.citations.as_mut() { for citation in citations {
            if citation.note_id==old { citation.note_id=next.into(); citation.knowledge_base_id=kb.into(); citation.knowledge_base_name=name.into(); citation.path=path.into(); }
        } }
    }
}
