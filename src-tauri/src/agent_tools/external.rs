use super::types::*;
use serde_json::{json, Value};

pub(crate) struct ExecTool;
impl AgentTool for ExecTool {
    fn name(&self) -> &'static str { "exec" }
    fn description(&self) -> &'static str { "Execute a command in a new non-interactive shell as the current OS user. Real environment access, no filesystem sandbox. No persistent shell state, interactive input, background services or elevation. The host declares the actual shell in context. Every full-confirm call requires approval." }
    fn parameters(&self) -> Value { json!({"type":"object", "properties":{
        "command":{"type":"string"}, "cwd":{"type":"string","description":"Defaults to this session's active knowledge-base root; never to an arbitrary host directory."},
        "timeoutSeconds":{"type":"integer","minimum":5,"maximum":1800}}, "required":["command"], "additionalProperties":false}) }
    fn execute(&self, _: &mut AgentToolContext<'_>, _: &Value) -> ToolExecutionResult { ToolExecutionResult::failed("exec 必须由异步 Runtime 派发。") }
}

/** 分类与路径策略在 schema 和实际派发中共用。不得先读取外部正文再请求审批。 */
pub(crate) fn decision(snapshot: &crate::domain::WorkspaceSnapshot, index: usize, name: &str, args: &Value) -> Result<crate::permissions::Decision, String> {
    use crate::permissions::{self, Decision, Operation};
    let session = &snapshot.sessions[index];
    let initial = permissions::decide(session, permissions::tool_operation(name,args));
    if matches!(initial, Decision::Deny(_)) { return Ok(initial); }
    if name == "read" && args["target"] == "skill" { return Ok(initial); }
    // edit/write 优先解析 ID；read 则优先 path，list/search 完全不使用 ID。
    // 不能用一个范围内的 fileId 掩盖读取工具实际采用的外部 path。
    let identified = matches!(name, "edit" | "write").then(|| args["fileId"].as_str().or(args["noteId"].as_str())).flatten().and_then(|id| {
        snapshot.notes.iter().find(|n| n.id == id).map(|n| (&n.knowledge_base_id, &n.path))
            .or_else(|| snapshot.documents.iter().find(|d| d.id == id).map(|d| (&d.knowledge_base_id, &d.path)))
    });
    if let Some((kb, _)) = identified {
        if !session.knowledge_base_ids.contains(kb) { return Err("文件 ID 不在会话范围内；完全权限可改用绝对路径并审批外部读取。".into()); }
        return Ok(initial);
    }
    let path = identified.map(|(_, path)| path.as_str()).or(args["path"].as_str()).or(args["targetPath"].as_str());
    if let Some(path) = path {
        // 合法相对路径只可能落在所选根内；具体存在性、链接和读取错误由执行入口验证。
        if !std::path::Path::new(path).is_absolute() && !path.starts_with('~') {
            if args["knowledgeBaseId"].as_str().is_some_and(|id| !session.knowledge_base_ids.iter().any(|kb| kb == id)) {
                return Err("目标知识库不在当前会话允许范围内。".into());
            }
            return Ok(initial);
        }
        let active = session.knowledge_base_ids.first().ok_or("会话没有激活知识库")?;
        let target=crate::fs_guard::resolve_agent_fs_target(&snapshot.knowledge_bases,&session.knowledge_base_ids,
            active,&session.security_level,identified.map(|(kb, _)| kb.as_str()).or(args["knowledgeBaseId"].as_str()),path,false)?;
        if target.is_external() {
            // 外部新建只需要最终文件变更审批；准备 edit 的正文/清单必须先审批读取。
            if name == "write" { permissions::ensure(session,Operation::ExternalRead)?; }
            else { return Ok(permissions::decide(session,Operation::ExternalRead)); }
        }
    }
    Ok(initial)
}
