//! 会话权限的唯一策略入口。未知值收权；旧值仅在迁移时映射，执行时不兼容升权。
use crate::domain::{AgentSecurityLevel, AgentSession};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation { Read, ProposeText, ManageFiles, Apply, ExternalRead, ExternalTool, Task }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision { Allow, RequireApproval, Deny(&'static str) }

/** Plan 与来源限制优先于权限档位，审批无法放行 deny。 */
pub fn decide(session: &AgentSession, operation: Operation) -> Decision {
    use Decision::*;
    use Operation::*;
    let level = AgentSecurityLevel::parse(&session.security_level);
    let readonly = session.plan_mode || session.schedule_identity.is_some()
        || level == AgentSecurityLevel::WorkspaceReadonly;
    if matches!(operation, Read | Task) { return Allow; }
    if readonly { return Deny("当前为工作区只读或 Plan 模式，请由用户切换权限后重试。"); }
    if session.im_identity.is_some() {
        return if matches!(operation, ProposeText | Apply) { Allow }
        else { Deny("IM 仅支持工作区文本草稿及身份绑定确认，不允许文件管理和外部执行。") };
    }
    match operation {
        ProposeText | ManageFiles | Apply => Allow,
        ExternalRead | ExternalTool => match level {
            AgentSecurityLevel::FullConfirm => RequireApproval,
            AgentSecurityLevel::FullAuto => Allow,
            _ => Deny("此操作需要完全权限；请由用户切换为完全权限，需审批或无需审批。"),
        },
        _ => Allow,
    }
}

/** 兼容迁移独立于执行判定，重复执行不改变新档位。 */
pub fn migrate_level(value: &str) -> String {
    match value {
        "basic" | "advanced" | "autonomous" | "full" => "workspace-write",
        "workspace-readonly" | "workspace-write" | "full-confirm" | "full-auto" => value,
        _ => "workspace-readonly",
    }.to_owned()
}

pub fn ensure(session: &AgentSession, operation: Operation) -> Result<(), String> {
    match decide(session, operation) { Decision::Deny(reason) => Err(reason.to_owned()), _ => Ok(()) }
}

/** 工具 schema 与执行共用分类；路径是否越界由规范化后的真实路径确定。 */
pub fn tool_operation(name: &str, args: &Value) -> Operation {
    match name {
        "exec" | "browser" => Operation::ExternalTool,
        name if name.starts_with("mcp__") => Operation::ExternalTool,
        "search" if args["target"].as_str().is_some_and(|target| target.trim().eq_ignore_ascii_case("web")) => Operation::ExternalTool,
        "read" if args.get("url").is_some() || args["target"] == "url" => Operation::ExternalTool,
        "edit" if matches!(args["operation"].as_str(), Some("move" | "rename" | "delete")) => Operation::ManageFiles,
        "write" if crate::agent_tools::write_kind(args) == "folder" => Operation::ManageFiles,
        "edit" | "write" => Operation::ProposeText,
        "task" => Operation::Task,
        _ => Operation::Read,
    }
}

/** 权限更新入口与执行共用指纹，绑定 Plan、scope 和持久化版本。 */
pub fn same_authority(a: &AgentSession, b: &AgentSession) -> bool {
    a.id == b.id && a.permission_version == b.permission_version
        && a.security_level == b.security_level && a.plan_mode == b.plan_mode
        && a.knowledge_base_ids == b.knowledge_base_ids
        && a.im_identity.is_some() == b.im_identity.is_some()
        && a.schedule_identity.is_some() == b.schedule_identity.is_some()
}

/** 权限与持久化回归测试共用最小会话，不依赖真实用户数据库。 */
#[cfg(test)]
pub(crate) fn test_session(level: &str) -> AgentSession {
    serde_json::from_value(serde_json::json!({
        "id":"permission-test", "title":"测试", "type":"knowledge-base", "knowledgeBaseIds":["kb-a"],
        "activeNoteId":null,"pinnedNoteIds":[],"messages":[],"securityLevel":level,"planMode":false,
        "createdAt":"2026/10/26 10:00","updatedAt":"2026/10/26 10:00"
    })).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_aliases_keep_the_same_external_and_management_gates() {
        for target in ["web", "WEB", " Web "] {
            assert_eq!(tool_operation("search", &serde_json::json!({"target":target})), Operation::ExternalTool);
        }
        for args in [serde_json::json!({"kind":"folder"}), serde_json::json!({"kind":" FOLDER "}),
            serde_json::json!({"fileType":"folder"}), serde_json::json!({"file_type":"Folder"})] {
            assert_eq!(tool_operation("write", &args), Operation::ManageFiles);
        }
    }

    #[test]
    fn migration_is_idempotent_and_unknown_cannot_escalate() {
        for old in ["basic", "advanced", "autonomous", "full"] { assert_eq!(migrate_level(old), "workspace-write"); }
        for level in ["workspace-readonly", "workspace-write", "full-confirm", "full-auto", "oops"] {
            assert_eq!(migrate_level(&migrate_level(level)), migrate_level(level));
        }
        assert_eq!(AgentSecurityLevel::parse("autonomous"), AgentSecurityLevel::WorkspaceReadonly);
    }

    #[test]
    fn four_levels_plan_and_remote_sources_follow_operation_matrix() {
        for level in ["workspace-readonly", "workspace-write", "full-confirm", "full-auto", "invalid", "autonomous"] {
            for plan in [false,true] {
                for source in ["local","im","schedule"] {
                    let mut session = test_session(level); session.plan_mode = plan;
                    if source == "im" { session.im_identity = Some(serde_json::from_value(serde_json::json!({
                        "providerId":"feishu","conversationKind":"direct","channelHash":"test","initialMessagePreview":"","lastMessagePreview":""
                    })).unwrap()); }
                    if source == "schedule" { session.schedule_identity = Some(serde_json::from_value(serde_json::json!({
                        "jobId":"s","jobName":"测试"
                    })).unwrap()); }
                    let readonly = plan || source == "schedule" || matches!(level,"workspace-readonly"|"invalid"|"autonomous");
                    for operation in [Operation::Read,Operation::Task] { assert_eq!(decide(&session,operation),Decision::Allow); }
                    for operation in [Operation::ProposeText,Operation::Apply,Operation::ManageFiles,Operation::ExternalRead,Operation::ExternalTool] {
                        let expected_deny = readonly || (source == "im" && !matches!(operation,Operation::ProposeText|Operation::Apply))
                            || (matches!(operation,Operation::ExternalRead|Operation::ExternalTool) && level == "workspace-write");
                        assert_eq!(matches!(decide(&session,operation),Decision::Deny(_)),expected_deny,"{level} {plan} {source} {operation:?}");
                        if !expected_deny && source == "local" && level == "full-confirm" && matches!(operation,Operation::ExternalRead|Operation::ExternalTool) {
                            assert_eq!(decide(&session,operation),Decision::RequireApproval);
                        }
                    }
                }
            }
        }
    }
}
