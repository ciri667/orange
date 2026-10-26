use crate::{agent_tools::ToolOutcome, domain::*, storage, runtime::AgentCancel};
use serde_json::{json, Value};
use tauri::AppHandle;

pub(super) fn outcome(name: &str, args: &Value, success: bool, payload: Value, summary: String) -> ToolOutcome {
    ToolOutcome { call: AgentToolCall { id: storage::create_id("tool"), name: name.into(), status: if success {"completed"} else {"failed"}.into(), summary, args: args.clone() }, payload, citations:vec![],audit_fragment:None }
}
pub(super) fn failed(name: &str,args: &Value,message: &str) -> ToolOutcome { outcome(name,args,false,json!({"error":message}),message.into()) }

/** 审批前固定默认 cwd 与超时，用户看到的参数就是执行入口收到的参数。 */
pub(super) fn prepare_exec(app: &AppHandle, snapshot: &WorkspaceSnapshot, index: usize, args: &Value) -> Result<Value,String> {
    let session=&snapshot.sessions[index];
    crate::permissions::ensure(session,crate::permissions::Operation::ExternalTool)?;
    let command=args["command"].as_str().filter(|s|!s.trim().is_empty() && !s.contains('\0')).ok_or("exec 缺少有效 command")?;
    let active=session.knowledge_base_ids.first().ok_or("会话没有激活知识库，不能选择默认 cwd")?;
    let cwd=if let Some(path)=args["cwd"].as_str() {
        crate::fs_guard::resolve_agent_fs_target(&snapshot.knowledge_bases,&session.knowledge_base_ids,active,&session.security_level,None,path,true)?.absolute_path
    } else {
        let kb=snapshot.knowledge_bases.iter().find(|k|&k.id==active).ok_or("激活知识库已失效")?;
        std::fs::canonicalize(&kb.path).map_err(|e|e.to_string())?
    };
    if !cwd.is_dir() { return Err("cwd 必须为有效目录。".into()); }
    let timeout=match args.get("timeoutSeconds") {
        Some(value)=>value.as_u64().filter(|v|(5..=1800).contains(v)).ok_or("timeoutSeconds 必须为 5–1800 的整数")?,
        None=>storage::load_user_settings(app)?.agent_security.resource_limits.timeout_seconds,
    };
    Ok(json!({"command":command,"cwd":cwd,"timeoutSeconds":timeout}))
}

pub(super) async fn exec(app: &AppHandle, snapshot: &mut WorkspaceSnapshot, index: usize, args: &Value, cancel: &AgentCancel, permit: &crate::operation_approval::Permit) -> ToolOutcome {
    let result = async {
        let command=args["command"].as_str().ok_or("exec 缺少 command")?;
        let session=&snapshot.sessions[index];
        permit.validate(app,session,"exec",args,cancel)?;
        let active=session.knowledge_base_ids.first().ok_or("会话没有激活知识库；不能选择默认 cwd")?;
        let cwd=if let Some(path)=args["cwd"].as_str() { crate::fs_guard::resolve_agent_fs_target(&snapshot.knowledge_bases,&session.knowledge_base_ids,active,&session.security_level,None,path,true)?.absolute_path }
            else { let kb=snapshot.knowledge_bases.iter().find(|k|&k.id==active).ok_or("激活知识库已失效")?; std::fs::canonicalize(&kb.path).map_err(|e|e.to_string())? };
        let settings=storage::load_user_settings(app)?;
        let timeout=match args.get("timeoutSeconds") { None=>settings.agent_security.resource_limits.timeout_seconds,Some(v)=>v.as_u64().ok_or("timeoutSeconds 必须为整数")? };
        crate::command_exec::execute(command,&cwd,timeout,cancel).await
    }.await;
    // 成功、失败或取消都可能已修改磁盘；不能继续使用调用前的索引或正文。
    let refresh=crate::file_changes::refresh(app,snapshot);
    let mut output=match result {
        Ok(result)=> { let success=result.success(); let summary=if success {"命令执行完成"} else if result.cancelled {"命令已取消，可能已有副作用"} else if result.timed_out {"命令超时，可能已有副作用"} else {"命令执行失败"}; outcome("exec",args,success,json!(result),summary.into()) },
        Err(e)=>failed("exec",args,&e),
    };
    if let Err(e)=refresh { output.call.status="failed".into(); output.payload["rescanError"]=json!(e); output.call.summary.push_str("；知识库重扫失败，请刷新后再继续"); }
    output
}
