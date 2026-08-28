mod execute;
mod registry;
mod types;

pub use registry::ToolRegistry;
pub(crate) use registry::{model_tool_call_name, parse_tool_args};
pub use types::{AgentToolContext, ToolOutcome};

#[cfg(test)]
pub(crate) use types::MAX_READ_NOTE_CHARS;

#[cfg(test)]
mod tests {
    use super::types::MAX_TREE_ITEMS;
    use super::*;
    use crate::domain::{
        AgentSecuritySettings, AgentSession, AgentTurnRequest, FolderEntry, KnowledgeBase, Note,
        WorkspaceDocument, WorkspaceSnapshot,
    };
    use crate::storage::hash_content;
    use serde_json::json;

    /** 构造工具层测试使用的最小工作台快照。 */
    fn tool_test_snapshot(note_content: String) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            knowledge_bases: vec![
                KnowledgeBase {
                    id: "kb-a".to_owned(),
                    name: "主知识库".to_owned(),
                    path: "/tmp/kb-a".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: true,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
                KnowledgeBase {
                    id: "kb-b".to_owned(),
                    name: "未授权知识库".to_owned(),
                    path: "/tmp/kb-b".to_owned(),
                    description: "测试知识库".to_owned(),
                    status: "ready".to_owned(),
                    note_count: 1,
                    document_count: 0,
                    updated_at: "刚刚".to_owned(),
                    is_default: false,
                    semantic_index_enabled: false,
                    scan_report: None,
                },
            ],
            folders: vec![FolderEntry {
                id: "folder-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                name: "Notes".to_owned(),
                path: "Notes".to_owned(),
                updated_at: "刚刚".to_owned(),
            }],
            notes: vec![
                Note {
                    id: "note-a".to_owned(),
                    knowledge_base_id: "kb-a".to_owned(),
                    title: "授权笔记".to_owned(),
                    path: "Notes/授权笔记.md".to_owned(),
                    content_hash: hash_content(&note_content),
                    content: note_content,
                    tags: vec!["测试".to_owned()],
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
                Note {
                    id: "note-b".to_owned(),
                    knowledge_base_id: "kb-b".to_owned(),
                    title: "未授权笔记".to_owned(),
                    path: "Private/未授权笔记.md".to_owned(),
                    content_hash: hash_content("private"),
                    content: "private".to_owned(),
                    tags: Vec::new(),
                    updated_at: "刚刚".to_owned(),
                    backlinks: Vec::new(),
                },
            ],
            documents: Vec::new(),
            sessions: vec![AgentSession {
                id: "session-a".to_owned(),
                title: "测试会话".to_owned(),
                im_identity: None,
                r#type: "knowledge-base".to_owned(),
                knowledge_base_ids: vec!["kb-a".to_owned()],
                active_note_id: Some("note-a".to_owned()),
                pinned_note_ids: vec!["note-a".to_owned()],
                messages: Vec::new(),
                pending_change: None,
                pending_change_set: None,
                pending_execution: None,
                security_level: "basic".to_owned(),
                context_summary: None,
                created_at: "刚刚".to_owned(),
                updated_at: "刚刚".to_owned(),
                deleted_at: None,
                model_provider_id: None,
                model_id: None,
                context_usage: None,
            }],
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            active_document_id: String::new(),
            active_session_id: "session-a".to_owned(),
        }
    }

    /** 构造工具层测试使用的 Agent 请求。 */
    fn tool_test_request(action: &str, prompt: &str) -> AgentTurnRequest {
        AgentTurnRequest {
            prompt: prompt.to_owned(),
            action: action.to_owned(),
            session_id: "session-a".to_owned(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: "note-a".to_owned(),
            client_message_id: None,
            model_provider_id: None,
            model_id: None,
            explicit_skill_ids: Vec::new(),
            mentioned_file_ids: Vec::new(),
        }
    }

    /** 创建无 AppHandle 的纯内存工具上下文，适合测试非索引类工具。 */
    fn tool_test_context<'a>(
        snapshot: &'a mut WorkspaceSnapshot,
        request: &'a AgentTurnRequest,
    ) -> AgentToolContext<'a> {
        AgentToolContext {
            app: None,
            snapshot,
            session_index: 0,
            request,
        }
    }

    /** 构造工具层普通文档条目，测试 list_tree 元数据时不需要真实文件系统。 */
    fn tool_test_document(
        id: &str,
        knowledge_base_id: &str,
        path: &str,
        file_type: &str,
        preview_available: bool,
    ) -> WorkspaceDocument {
        WorkspaceDocument {
            id: id.to_owned(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            title: path
                .rsplit('/')
                .next()
                .unwrap_or("测试文档")
                .trim_end_matches(&format!(".{file_type}"))
                .to_owned(),
            path: path.to_owned(),
            file_type: file_type.to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: hash_content(id),
            content: (file_type == "txt").then(|| "纯文本正文不会通过 list_tree 返回。".to_owned()),
            preview_available,
        }
    }

    /** 默认 registry 只暴露闭集五个短名。 */
    #[test]
    fn registry_schema_contains_builtin_tools() {
        let registry = ToolRegistry::default();
        let schemas = registry.schemas();
        let tool_names = registry.tool_names();

        assert!(schemas.is_array());
        assert_eq!(tool_names, vec!["search", "read", "list", "edit", "write"]);
        assert!(!tool_names.contains(&"run"));
        assert!(!tool_names.contains(&"search_notes"));
        assert!(!tool_names.contains(&"read_document"));
        assert!(!tool_names.contains(&"get_current_file"));
        assert!(!tool_names.contains(&"get_session_summary"));
        assert!(!tool_names.contains(&"get_knowledge_base_memory"));
        assert!(!tool_names.contains(&"suggest_organization"));
        assert!(!tool_names.contains(&"search_session_messages"));
        assert!(!tool_names.contains(&"read_session_context"));
    }

    /** run_skill 仅在本地进阶/完全会话和全局执行开关同时满足时注册；create_folder 同样要求本地进阶会话。 */
    #[test]
    fn run_skill_registry_requires_local_advanced_session() {
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;

        snapshot.sessions[0].security_level = "advanced".to_owned();
        let advanced_tools =
            ToolRegistry::for_session(&snapshot.sessions[0], &settings).tool_names();
        assert!(advanced_tools.contains(&"run"));
        assert!(!advanced_tools.contains(&"create_folder"));
        assert!(!advanced_tools.contains(&"list_path"));
        assert!(!advanced_tools.contains(&"read_path"));
        assert!(!advanced_tools.contains(&"run_skill"));
        assert_eq!(
            advanced_tools,
            vec!["search", "read", "list", "edit", "write", "run"]
        );

        snapshot.sessions[0].security_level = "basic".to_owned();
        let basic_tools = ToolRegistry::for_session(&snapshot.sessions[0], &settings).tool_names();
        assert!(!basic_tools.contains(&"run"));
        assert!(!basic_tools.contains(&"create_folder"));
        assert!(!basic_tools.contains(&"list_path"));
        assert_eq!(basic_tools, vec!["search", "read", "list", "edit", "write"]);

        snapshot.sessions[0].security_level = "autonomous".to_owned();
        settings.autonomous_mode_enabled = false;
        let autonomous_without_toggle =
            ToolRegistry::for_session(&snapshot.sessions[0], &settings).tool_names();
        assert!(!autonomous_without_toggle.contains(&"run"));
        assert!(!autonomous_without_toggle.contains(&"create_folder"));
        assert!(!autonomous_without_toggle.contains(&"list_path"));
        assert!(!autonomous_without_toggle.contains(&"read_path"));
        assert_eq!(
            autonomous_without_toggle,
            vec!["search", "read", "list", "edit", "write"]
        );

        snapshot.sessions[0].security_level = "advanced".to_owned();
        snapshot.sessions[0].im_identity = Some(crate::domain::ImSessionIdentity {
            provider_id: "feishu".to_owned(),
            conversation_kind: "direct".to_owned(),
            channel_hash: "redacted".to_owned(),
            initial_message_preview: "IM".to_owned(),
            last_message_preview: "IM".to_owned(),
        });
        let im_tools = ToolRegistry::for_session(&snapshot.sessions[0], &settings).tool_names();
        assert!(!im_tools.contains(&"run"));
        assert_eq!(im_tools, vec!["search", "read", "list", "edit", "write"]);
    }

    /** 任何级别 schema 都不应再出现分身工具名。 */
    #[test]
    fn closed_set_schema_never_exposes_split_tool_names() {
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        settings.autonomous_mode_enabled = true;
        for level in ["basic", "advanced", "autonomous"] {
            snapshot.sessions[0].security_level = level.to_owned();
            let names = ToolRegistry::for_session(&snapshot.sessions[0], &settings).tool_names();
            for forbidden in [
                "create_folder",
                "list_path",
                "read_path",
                "run_skill",
                "search_notes",
                "read_file",
                "propose_file_change",
                "create_file_draft",
            ] {
                assert!(
                    !names.contains(&forbidden),
                    "level={level} still exposes {forbidden}"
                );
            }
        }
    }

    /** create_folder 在 advanced 会话执行后应生成 agent-direct 变更集，operation 类型为 create_folder。 */
    #[test]
    fn create_folder_builds_agent_direct_change_set() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        snapshot.sessions[0].security_level = "advanced".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("create", "建文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "Notes/新目录" }),
        );

        assert_eq!(outcome.call.status, "completed");
        let change_set = context.snapshot.sessions[0]
            .pending_change_set
            .as_ref()
            .unwrap();
        assert_eq!(change_set.execution_id, "agent-direct");
        assert_eq!(change_set.status, "pending");
        assert_eq!(change_set.operations.len(), 1);
        assert_eq!(change_set.operations[0].operation, "create_folder");
        assert_eq!(change_set.operations[0].file_type, "folder");
        assert_eq!(change_set.operations[0].target_path, "Notes/新目录");
        assert!(!change_set.operations[0].binary);
    }

    /** 连续两次 create_folder 应追加到同一 agent-direct 变更集。 */
    #[test]
    fn create_folder_appends_to_existing_agent_change_set() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        snapshot.sessions[0].security_level = "advanced".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("create", "建文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "Notes/目录A" }),
        );
        registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "Notes/目录B" }),
        );

        let change_set = context.snapshot.sessions[0]
            .pending_change_set
            .as_ref()
            .unwrap();
        assert_eq!(change_set.operations.len(), 2);
        assert_eq!(change_set.operations[0].target_path, "Notes/目录A");
        assert_eq!(change_set.operations[1].target_path, "Notes/目录B");
    }

    /** create_folder 必须拒绝路径穿越和 scope 外知识库。 */
    #[test]
    fn create_folder_rejects_unsafe_paths_and_outside_scope() {
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.sessions[0].security_level = "advanced".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("create", "建文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);

        let escape_outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "../escape" }),
        );
        assert_eq!(escape_outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change_set.is_none());

        let absolute_outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "/etc/evil" }),
        );
        assert_eq!(absolute_outcome.call.status, "failed");

        let outside_scope_outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "knowledgeBaseId": "kb-b", "targetPath": "Notes/目录" }),
        );
        assert_eq!(outside_scope_outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change_set.is_none());
    }

    /** 完全级别允许合规的绝对路径，并把它记为 external scope。 */
    #[test]
    fn create_folder_full_mode_accepts_compliant_absolute_path() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let external = tempfile::tempdir().expect("external root");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        snapshot.sessions[0].security_level = "autonomous".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        settings.autonomous_mode_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("create", "建外部文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        let target = external.path().join("AgentOut");
        let outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": target.to_string_lossy() }),
        );

        assert_eq!(outcome.call.status, "completed");
        let operation = &context.snapshot.sessions[0]
            .pending_change_set
            .as_ref()
            .unwrap()
            .operations[0];
        assert_eq!(operation.knowledge_base_id, "external");
        assert!(operation.target_path.contains("AgentOut"));
    }

    /** 完全级别下，落在授权知识库内的绝对路径仍按知识库相对路径保存。 */
    #[test]
    fn create_folder_full_mode_maps_absolute_path_inside_kb() {
        let kb_root = tempfile::tempdir().expect("kb root");
        std::fs::create_dir_all(kb_root.path().join("Notes")).expect("notes");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        snapshot.sessions[0].security_level = "autonomous".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        settings.autonomous_mode_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("create", "建知识库文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        let target = kb_root.path().join("Notes").join("归档");
        let outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": target.to_string_lossy() }),
        );

        assert_eq!(outcome.call.status, "completed");
        let operation = &context.snapshot.sessions[0]
            .pending_change_set
            .as_ref()
            .unwrap()
            .operations[0];
        assert_eq!(operation.knowledge_base_id, "kb-a");
        assert_eq!(operation.target_path, "Notes/归档");
    }

    /** list_path / read_path 可读取知识库外的合规目录和文本。 */
    #[test]
    fn list_and_read_path_full_mode_round_trip() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let external = tempfile::tempdir().expect("external root");
        std::fs::write(external.path().join("hello.txt"), "hello orange").expect("write file");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        snapshot.sessions[0].security_level = "autonomous".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        settings.autonomous_mode_enabled = true;
        let registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let request = tool_test_request("ask", "看外部目录");
        let mut context = tool_test_context(&mut snapshot, &request);

        let list_outcome = registry.execute_named(
            &mut context,
            "list_path",
            json!({ "path": external.path().to_string_lossy() }),
        );
        assert_eq!(list_outcome.call.status, "completed");
        assert!(list_outcome.payload["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "hello.txt"));

        let read_outcome = registry.execute_named(
            &mut context,
            "read_path",
            json!({ "path": external.path().join("hello.txt").to_string_lossy() }),
        );
        assert_eq!(read_outcome.call.status, "completed");
        assert!(read_outcome.payload["content"]
            .as_str()
            .unwrap()
            .contains("hello orange"));
        assert_eq!(read_outcome.payload["external"], true);
    }

    /** 完全级别 search target=path 能命中目录文本；基础级别同一 path 失败。 */
    #[test]
    fn search_path_full_mode_hits_and_basic_rejects() {
        let kb_root = tempfile::tempdir().expect("kb root");
        let external = tempfile::tempdir().expect("external root");
        std::fs::write(external.path().join("notes.txt"), "orange secret token")
            .expect("write file");
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.knowledge_bases[0].path = kb_root.path().to_string_lossy().into_owned();
        let request = tool_test_request("ask", "搜外部");
        let basic_registry = ToolRegistry::default();
        let basic_outcome = {
            let mut basic_context = tool_test_context(&mut snapshot, &request);
            basic_registry.execute_named(
                &mut basic_context,
                "search",
                json!({
                    "query": "secret",
                    "target": "path",
                    "path": external.path().to_string_lossy()
                }),
            )
        };
        assert_eq!(basic_outcome.call.status, "failed");

        snapshot.sessions[0].security_level = "autonomous".to_owned();
        let mut settings = AgentSecuritySettings::default();
        settings.advanced_execution_enabled = true;
        settings.autonomous_mode_enabled = true;
        let full_registry = ToolRegistry::for_session(&snapshot.sessions[0], &settings);
        let mut full_context = tool_test_context(&mut snapshot, &request);
        let full_outcome = full_registry.execute_named(
            &mut full_context,
            "search",
            json!({
                "query": "secret",
                "target": "path",
                "path": external.path().to_string_lossy()
            }),
        );
        assert_eq!(full_outcome.call.status, "completed");
        assert_eq!(full_outcome.call.name, "search");
        let hits = full_outcome.payload["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0]["snippet"]
            .as_str()
            .unwrap_or_default()
            .contains("secret"));
    }

    /** 基础级别 write 建文件夹必须失败且不产生 pending。 */
    #[test]
    fn write_folder_rejects_basic_session() {
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let registry = ToolRegistry::default();
        let request = tool_test_request("create", "建文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "write",
            json!({ "kind": "folder", "targetPath": "Notes/新目录" }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert_eq!(outcome.call.name, "write");
        assert!(context.snapshot.sessions[0].pending_change_set.is_none());
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 基础级别 list/read 外部 path 必须失败。 */
    #[test]
    fn list_and_read_external_path_reject_basic_session() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "看外部路径");
        let mut context = tool_test_context(&mut snapshot, &request);
        let list_outcome =
            registry.execute_named(&mut context, "list", json!({ "path": "C:/Windows" }));
        let read_outcome = registry.execute_named(
            &mut context,
            "read",
            json!({ "path": "C:/Windows/win.ini" }),
        );

        assert_eq!(list_outcome.call.status, "failed");
        assert_eq!(read_outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** basic 会话即使手动调用 create_folder 也必须失败。 */
    #[test]
    fn create_folder_rejects_basic_session() {
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        // 会话保持 basic；模拟模型误调用。这里用 default registry，因为 for_session 在 basic 不会注册 create_folder。
        let registry = ToolRegistry::default();
        let request = tool_test_request("create", "建文件夹");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "create_folder",
            json!({ "targetPath": "Notes/新目录" }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change_set.is_none());
    }

    /** 未知工具调用必须失败且不能修改 pending_change。 */
    #[test]
    fn unknown_tool_is_rejected_without_pending_change() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "测试未知工具");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "unknown_tool", json!({}));

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 无 fileId 时 read 读取当前激活笔记。 */
    #[test]
    fn read_without_id_uses_active_file() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("当前激活笔记正文。".to_owned());
        let request = tool_test_request("ask", "读当前文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "read", json!({}));

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(outcome.call.name, "read");
        assert_eq!(
            outcome.payload["note"]["content"].as_str(),
            Some("当前激活笔记正文。")
        );
    }

    /** 旧名 remap 后仍能执行，轨迹使用闭集短名。 */
    #[test]
    fn legacy_tool_names_remap_to_closed_set() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "兼容旧名");
        let mut context = tool_test_context(&mut snapshot, &request);

        let search = registry.execute_named(
            &mut context,
            "search_notes",
            json!({ "query": "不会命中因为没有 app" }),
        );
        assert_eq!(search.call.name, "search");

        let read = registry.execute_named(&mut context, "read_note", json!({ "noteId": "note-a" }));
        assert_eq!(read.call.name, "read");
        assert_eq!(read.call.status, "completed");

        let list = registry.execute_named(&mut context, "list_tree", json!({}));
        assert_eq!(list.call.name, "list");
        assert_eq!(list.call.status, "completed");
    }

    /** 已降级为宿主注入的工具不再出现在 schema，调用只返回结构化失败。 */
    #[test]
    fn retired_host_tools_fail_without_pending_change() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "旧宿主工具");
        let mut context = tool_test_context(&mut snapshot, &request);

        for name in [
            "get_session_summary",
            "get_knowledge_base_memory",
            "search_session_messages",
            "read_session_context",
            "suggest_organization",
        ] {
            let outcome = registry.execute_named(&mut context, name, json!({ "query": "x" }));
            assert_eq!(outcome.call.status, "failed", "{name}");
            assert!(outcome.payload.get("error").is_some(), "{name}");
        }
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** read_note 必须拒绝读取当前会话 scope 外的笔记。 */
    #[test]
    fn read_note_rejects_note_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("ask", "读取笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome =
            registry.execute_named(&mut context, "read_note", json!({ "noteId": "note-b" }));

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.payload.get("error").is_some());
    }

    /** list_tree 应返回当前 scope 内普通文档元数据，但不暴露正文和 hash。 */
    #[test]
    fn list_tree_returns_document_metadata_for_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.documents = vec![
            tool_test_document("document-txt", "kb-a", "Docs/brief.txt", "txt", false),
            tool_test_document("document-pdf", "kb-a", "Docs/spec.pdf", "pdf", true),
        ];
        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();
        let txt_document = documents
            .iter()
            .find(|document| document["id"].as_str() == Some("document-txt"))
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), 2);
        assert_eq!(txt_document["fileType"].as_str(), Some("txt"));
        assert_eq!(txt_document["knowledgeBaseId"].as_str(), Some("kb-a"));
        assert_eq!(txt_document["knowledgeBaseName"].as_str(), Some("主知识库"));
        assert_eq!(txt_document["previewAvailable"].as_bool(), Some(false));
        assert_eq!(txt_document["agentReadable"].as_bool(), Some(true));
        assert!(txt_document.get("content").is_none());
        assert!(txt_document.get("contentHash").is_none());
    }

    /** 统一读取和改写工具必须允许 scope 内 TXT，并保留纯文本正文。 */
    #[test]
    fn unified_tools_read_and_propose_txt_change() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("Markdown 正文".to_owned());
        let mut document =
            tool_test_document("document-txt", "kb-a", "Docs/brief.txt", "txt", false);
        document.content = Some("旧纯文本".to_owned());
        document.content_hash = hash_content("旧纯文本");
        snapshot.documents = vec![document];
        let request = tool_test_request("rewrite", "改写 TXT");
        let mut context = tool_test_context(&mut snapshot, &request);

        let read = registry.execute_named(
            &mut context,
            "read_file",
            json!({ "fileId": "document-txt" }),
        );
        assert_eq!(read.call.status, "completed");
        assert_eq!(read.payload["file"]["fileType"].as_str(), Some("txt"));
        assert_eq!(read.payload["file"]["content"].as_str(), Some("旧纯文本"));

        let change = registry.execute_named(&mut context, "propose_file_change", json!({
            "fileId": "document-txt", "operation": "replace", "original": "旧纯文本", "next": "新纯文本"
        }));
        assert_eq!(change.call.status, "completed");
        let pending = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();
        assert_eq!(pending.file_type.as_deref(), Some("txt"));
        assert_eq!(pending.target_kind.as_deref(), Some("document"));
        assert_eq!(pending.next, "新纯文本");
    }

    /** 同一文件第二次 edit 不得覆盖已有 pending。 */
    #[test]
    fn edit_same_file_rejects_when_pending_exists() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = tool_test_request("rewrite", "改两次");
        let mut context = tool_test_context(&mut snapshot, &request);
        let first = registry.execute_named(
            &mut context,
            "edit",
            json!({
                "fileId": "note-a",
                "operation": "replace",
                "original": "这是一段可以被改写的正文内容。",
                "next": "第一版"
            }),
        );
        let second = registry.execute_named(
            &mut context,
            "edit",
            json!({
                "fileId": "note-a",
                "operation": "replace",
                "original": "这是一段可以被改写的正文内容。",
                "next": "第二版"
            }),
        );

        assert_eq!(first.call.status, "completed");
        assert_eq!(second.call.status, "failed");
        assert_eq!(
            context.snapshot.sessions[0]
                .pending_change
                .as_ref()
                .unwrap()
                .next,
            "第一版"
        );
    }

    /** list_tree 必须按会话 scope 过滤普通文档，避免暴露未授权知识库结构。 */
    #[test]
    fn list_tree_rejects_documents_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        snapshot.documents = vec![
            tool_test_document("document-a", "kb-a", "Docs/allowed.txt", "txt", false),
            tool_test_document("document-b", "kb-b", "Private/hidden.pdf", "pdf", true),
        ];
        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0]["id"].as_str(), Some("document-a"));
        assert_eq!(outcome.payload["totalDocuments"].as_u64(), Some(1));
    }

    /** list_tree 应汇总混合文件总数、类型计数和截断状态。 */
    #[test]
    fn list_tree_reports_totals_type_counts_and_truncation() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());

        snapshot.documents = vec![
            tool_test_document("document-txt-base", "kb-a", "Docs/base.txt", "txt", false),
            tool_test_document("document-docx", "kb-a", "Docs/brief.docx", "docx", true),
            tool_test_document("document-pdf", "kb-a", "Docs/spec.pdf", "pdf", true),
            tool_test_document(
                "document-image",
                "kb-a",
                "Assets/diagram.png",
                "image",
                true,
            ),
        ];

        for index in 0..(MAX_TREE_ITEMS - 3) {
            // 生成超过 list_tree 单类预算的 TXT 文档，用于验证 totals 保留真实数量而数组被截断。
            snapshot.documents.push(tool_test_document(
                &format!("document-extra-{index}"),
                "kb-a",
                &format!("Docs/extra-{index}.txt"),
                "txt",
                false,
            ));
        }

        let request = tool_test_request("ask", "列出文件");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(&mut context, "list_tree", json!({}));
        let documents = outcome.payload["documents"].as_array().unwrap();
        let file_type_counts = &outcome.payload["fileTypeCounts"];

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(documents.len(), MAX_TREE_ITEMS);
        assert_eq!(outcome.payload["totalNotes"].as_u64(), Some(1));
        assert_eq!(
            outcome.payload["totalDocuments"].as_u64(),
            Some((MAX_TREE_ITEMS + 1) as u64)
        );
        assert_eq!(
            outcome.payload["totalFiles"].as_u64(),
            Some((MAX_TREE_ITEMS + 2) as u64)
        );
        assert_eq!(outcome.payload["truncated"].as_bool(), Some(true));
        assert!(outcome.payload["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("limit"));
        assert_eq!(file_type_counts["markdown"].as_u64(), Some(1));
        assert_eq!(
            file_type_counts["txt"].as_u64(),
            Some((MAX_TREE_ITEMS - 2) as u64)
        );
        assert_eq!(file_type_counts["docx"].as_u64(), Some(1));
        assert_eq!(file_type_counts["pdf"].as_u64(), Some(1));
        assert_eq!(file_type_counts["image"].as_u64(), Some(1));
    }

    /** read_note 会按上下文预算截断长正文并保留截断标记。 */
    #[test]
    fn read_note_truncates_large_content_for_model_context() {
        let registry = ToolRegistry::default();
        let long_content = "段落内容。".repeat(MAX_READ_NOTE_CHARS);
        let mut snapshot = tool_test_snapshot(long_content);
        let request = tool_test_request("ask", "读取长文");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome =
            registry.execute_named(&mut context, "read_note", json!({ "noteId": "note-a" }));
        let content = outcome.payload["note"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(outcome.payload["note"]["contentTruncated"], true);
        assert_eq!(outcome.payload["truncated"], true);
        assert!(outcome.payload["nextOffset"].as_u64().is_some());
        assert!(outcome.payload["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("offset="));
        assert!(!content.contains("内容已按上下文预算截断"));
    }

    /** 第二次带 offset 的 read 应续上且不重复前缀。 */
    #[test]
    fn read_offset_continues_without_repeating_prefix() {
        let registry = ToolRegistry::default();
        let content = format!("{}{}", "HEAD", "TAIL".repeat(MAX_READ_NOTE_CHARS));
        let mut snapshot = tool_test_snapshot(content);
        let request = tool_test_request("ask", "续读");
        let mut context = tool_test_context(&mut snapshot, &request);
        let first = registry.execute_named(&mut context, "read", json!({ "fileId": "note-a" }));
        let first_content = first.payload["note"]["content"]
            .as_str()
            .unwrap_or_default();
        let next_offset = first.payload["nextOffset"].as_u64().unwrap() as usize;

        assert!(first_content.starts_with("HEAD"));
        assert_eq!(first.payload["truncated"], true);

        let second = registry.execute_named(
            &mut context,
            "read",
            json!({ "fileId": "note-a", "offset": next_offset }),
        );
        let second_content = second.payload["note"]["content"]
            .as_str()
            .unwrap_or_default();

        assert!(!second_content.starts_with("HEAD"));
        assert!(second_content.contains("TAIL"));
        assert!(!second_content.contains("HEAD"));
    }

    /** rewrite 工具会拒绝无法命中原文的 diff，避免生成不可应用变更。 */
    #[test]
    fn propose_note_change_rejects_original_not_found() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "不存在的原文",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** rewrite 工具必须拒绝重复出现的 original，避免生成模糊 diff。 */
    #[test]
    fn propose_note_change_rejects_ambiguous_original() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("重复段落\n其他内容\n重复段落".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "重复段落",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.call.summary.contains("出现多次"));
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** rewrite 工具在 original 恰好命中一次时生成待确认 diff。 */
    #[test]
    fn propose_note_change_accepts_unique_original() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("第一段\n唯一段落\n第三段".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "original": "唯一段落",
                "next": "新的建议"
            }),
        );

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(
            context.snapshot.sessions[0]
                .pending_change
                .as_ref()
                .map(|change| change.original.as_str()),
            Some("唯一段落")
        );
    }

    /** 局部 original 不能搭配整篇文档 next，否则确认后会把前文重复插入。 */
    #[test]
    fn propose_note_change_rejects_full_document_next_for_partial_replace() {
        let registry = ToolRegistry::default();
        let original_content = "第一段\n第二段\n第三段";
        let mut snapshot = tool_test_snapshot(original_content.to_owned());
        let request = tool_test_request("rewrite", "在文末追加内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "replace",
                "original": "第二段",
                "next": format!("{}\n\n新增段落", original_content)
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(outcome.call.summary.contains("正文重复"));
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 文末追加必须使用 append，工具会把增量内容安全合成为整篇待确认 diff。 */
    #[test]
    fn propose_note_change_append_builds_full_note_replacement() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("第一段\n第二段".to_owned());
        let request = tool_test_request("rewrite", "在文末追加内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "append",
                "next": "新增段落"
            }),
        );

        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("append"));
        assert_eq!(change.original, "第一段\n第二段");
        assert_eq!(change.next, "第一段\n第二段\n\n新增段落");
    }

    /** 多处编辑应在工具层合成为整篇待确认 diff，避免模型拆成多个后续承诺。 */
    #[test]
    fn propose_note_change_multi_replace_builds_full_note_replacement() {
        let registry = ToolRegistry::default();
        let mut snapshot =
            tool_test_snapshot("标题\n重复段落一\n正文\n重复段落二\n结尾".to_owned());
        let request = tool_test_request("rewrite", "删除文档里的重复内容");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "multi_replace",
                "edits": [
                    { "original": "重复段落一\n", "next": "" },
                    { "original": "重复段落二\n", "next": "" }
                ]
            }),
        );
        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("multi_replace"));
        assert_eq!(change.original, "标题\n重复段落一\n正文\n重复段落二\n结尾");
        assert_eq!(change.next, "标题\n正文\n结尾");
    }

    /** 多处编辑支持 occurrence 精确删除重复片段中的指定一次。 */
    #[test]
    fn propose_note_change_multi_replace_accepts_occurrence_for_duplicates() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("开头\n重复段落\n中间\n重复段落\n结尾".to_owned());
        let request = tool_test_request("rewrite", "删除后面的重复段落");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({
                "noteId": "note-a",
                "operation": "multi_replace",
                "edits": [
                    { "original": "重复段落\n", "next": "", "occurrence": 2 }
                ]
            }),
        );
        let change = context.snapshot.sessions[0]
            .pending_change
            .as_ref()
            .unwrap();

        assert_eq!(outcome.call.status, "completed");
        assert_eq!(change.operation.as_deref(), Some("multi_replace"));
        assert_eq!(change.next, "开头\n重复段落\n中间\n结尾");
    }

    /** propose_note_change 必须拒绝 scope 外笔记。 */
    #[test]
    fn propose_note_change_rejects_note_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("这是一段可以被改写的正文内容。".to_owned());
        let request = tool_test_request("rewrite", "改写当前笔记");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "propose_note_change",
            json!({ "noteId": "note-b", "next": "新的建议" }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }

    /** 未授权知识库不能成为 create_note_draft 的目标。 */
    #[test]
    fn create_note_draft_rejects_knowledge_base_outside_scope() {
        let registry = ToolRegistry::default();
        let mut snapshot = tool_test_snapshot("正文内容足够用于测试。".to_owned());
        let request = tool_test_request("create", "生成草稿");
        let mut context = tool_test_context(&mut snapshot, &request);
        let outcome = registry.execute_named(
            &mut context,
            "create_note_draft",
            json!({
                "knowledgeBaseId": "kb-b",
                "targetPath": "Private/草稿.md",
                "content": "# 草稿"
            }),
        );

        assert_eq!(outcome.call.status, "failed");
        assert!(context.snapshot.sessions[0].pending_change.is_none());
    }
}
