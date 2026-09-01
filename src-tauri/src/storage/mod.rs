use crate::domain::{
    AgentSession, AppEventLog, DocumentHistoryEntry, DocumentHistoryEntryDetail, DocumentPreview,
    DocumentPreviewBlock, DocumentTextBlock, DocumentTextExtraction, FeishuCredentialStatus,
    FeishuIntegrationSettings, FolderEntry, ImIntegrationSettings, ImProviderConfig,
    ImProviderCredentialStatus, ImProviderSettings, KnowledgeBase, KnowledgeBaseMemory,
    KnowledgeBaseSelection, LlmProviderConfig, ModelApiKeyStatus, ModelConfig, Note,
    NoteImageAttachmentInput, RequestAuditLog, RevealedModelApiKey, SavedNoteImageAttachment,
    ScanReport, UserSettings, WorkspaceBootstrapState, WorkspaceDocument, WorkspaceEditorState,
    WorkspaceEditorTab, WorkspaceSnapshot, IM_PROVIDER_FEISHU, MEMORY_CATEGORY_CONVENTION,
    MEMORY_CATEGORY_NOTE_STRUCTURE, MEMORY_CATEGORY_ORGANIZATION, MEMORY_CATEGORY_OTHER,
    MEMORY_CATEGORY_TAG_CONVENTION, MEMORY_SOURCE_AUTO, MEMORY_SOURCE_USER,
};
use crate::model_provider;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chrono::{Duration as ChronoDuration, Local, NaiveDateTime, TimeZone};
use quick_xml::events::Event;
use quick_xml::Reader;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager};
use tempfile::NamedTempFile;
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};
use zip::ZipArchive;

/** 扫描时跳过的大型或生成目录，避免用户选到项目根目录后长时间遍历依赖和构建产物。 */
const IGNORED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vscode",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
];

/** 用户设置表中的默认记录 key，首版只有一个本机用户配置。 */
const USER_SETTINGS_KEY: &str = "default";

/** 即时通讯设置表 key；沿用 user_settings 表避免为单条本机配置新增表。 */
const IM_SETTINGS_KEY: &str = "im_integrations";

/** SQLite 被其他连接占用时的等待时长，覆盖大知识库索引重建的正常耗时窗口。 */
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/** 用户可读事件日志最多保留条数，避免 SQLite 随长期使用无限增长。 */
const MAX_APP_EVENT_LOGS: usize = 2_000;

/** 用户可读事件日志最长保留天数，和条数限制共同控制本地数据库体积。 */
const APP_EVENT_LOG_RETENTION_DAYS: i64 = 30;

/** 单个 Markdown/TXT 文件最多保留的历史版本数量。 */
const MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE: usize = 100;

/** 文档历史记录最长保留天数，超过后在下一次捕获或清空时清理。 */
const DOCUMENT_HISTORY_RETENTION_DAYS: i64 = 90;

/** 文档历史正文快照目录，位于 app data 下且文件名不包含用户路径。 */
const DOCUMENT_HISTORY_SNAPSHOT_DIR: &str = "document-history/v1";

/** 系统安全存储中的模型密钥引用，SQLite 只保存这个引用而不保存明文 key。 */
pub const MODEL_KEY_REFERENCE: &str = "orange-openai-compatible-api-key";

/** 系统安全存储中的飞书 appSecret 引用，SQLite 永远不保存明文 secret。 */
pub const FEISHU_SECRET_KEY_REFERENCE: &str = "orange-feishu-app-secret";

/** 正式构建使用的 Keychain service；生产用户保存的凭据只能由正式应用访问。 */
const PRODUCTION_KEYRING_SERVICE: &str = "Orange";

/** macOS debug 构建使用的独立 Keychain service，避免调试签名访问生产凭据。 */
const DEVELOPMENT_KEYRING_SERVICE: &str = "Orange Dev";

/** 单张粘贴图片最大字节数，避免超大剪贴板内容阻塞 UI 或撑爆本地目录。 */
const MAX_SINGLE_PASTE_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/** 单次粘贴图片总字节数上限，用于限制批量截图或多图复制的最坏写入成本。 */
const MAX_PASTE_IMAGE_BATCH_BYTES: usize = 50 * 1024 * 1024;

/** 图片附件文件名 hash 前缀长度，兼顾可读性和同秒重复粘贴冲突概率。 */
const PASTED_IMAGE_HASH_PREFIX_LENGTH: usize = 12;

/** 附件目录没有可用笔记名时使用的兜底目录名。 */
const DEFAULT_ATTACHMENT_NOTE_FOLDER_NAME: &str = "note";

/** 当前桌面进程内的模型密钥缓存，按 key 引用隔离，用于减少同一会话内反复访问系统安全存储。 */
static MODEL_API_KEY_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/** Keychain 命名空间只在进程首个访问时记录一次，避免高频读取产生重复日志。 */
static KEYRING_SERVICE_OBSERVABILITY: OnceLock<()> = OnceLock::new();

/** 当前桌面进程内的 SQLite 写锁，串行化索引刷新、会话保存和轻量迁移。 */
static DATABASE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/** 已完成 schema 初始化的 SQLite 文件路径，避免每次读命令都重复执行 DDL。 */
static INITIALIZED_DATABASE_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/** 最近一次已完成的 FTS 快照签名，用于跳过 StrictMode/reload 的重复索引任务。 */
static COMPLETED_INDEX_SIGNATURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

mod config;
mod db;
mod files;
mod history;
mod ids;
mod logs;
mod memory;
mod note_tags;
mod project_instructions;
mod rewind;
mod sessions;
mod workspace;

pub use config::*;
pub use db::*;
pub use files::*;
pub use history::*;
pub use ids::*;
pub use logs::*;
pub use memory::*;
pub use note_tags::*;
pub use project_instructions::*;
pub use rewind::*;
pub use sessions::*;
pub use workspace::*;

#[cfg(test)]
mod tests {
    use super::trash_markdown_file_with;
    use super::{
        atomic_write_markdown, atomic_write_text_document, create_blank_markdown_file,
        create_blank_text_document_file, create_folder, create_id, create_stable_note_id,
        ensure_database_schema, ensure_persistent_model_keyring, extract_document_text,
        extract_docx_preview_blocks, file_modified_local_datetime, format_local_datetime,
        format_local_datetime_from_millis, hash_bytes, hash_content, insert_app_event_log,
        insert_document_history_entry, is_missing_keyring_entry_error, keyring_service_for_build,
        load_agent_session_transcript_from_connection, load_document_history_ids_for_target,
        load_document_preview, load_keyring_password_after_cache_miss_with,
        load_latest_document_history_hash, load_model_api_key_from_cache,
        model_keyring_persists_until_delete, normalize_audit_log_created_at, normalize_im_settings,
        normalize_knowledge_base_memory, normalize_session_created_at,
        normalize_workspace_editor_state, parse_im_settings_payload,
        persist_agent_session_transcript, persist_session_in_transaction,
        persist_session_records_in_transaction, persist_sessions_in_transaction,
        prune_app_event_logs, prune_document_history_entries, query_app_event_logs,
        read_document_history_snapshot, redact_memory_secrets, rename_markdown_file,
        rename_text_document_file, resolve_existing_file_inside_root, resolve_inside_root,
        reveal_model_api_key, save_note_image_attachments, scan_markdown_directory,
        scan_supported_documents_directory, search_snapshot_notes,
        sort_sessions_by_updated_at_desc, store_model_api_key_in_cache, trash_markdown_file,
        trash_text_document_file_with, validate_folder_name, validate_markdown_file_name,
        validate_new_markdown_file_name, validate_new_text_document_file_name,
        validate_text_document_file_name, write_document_history_snapshot, BASE64_STANDARD,
        DEVELOPMENT_KEYRING_SERVICE, MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE,
        PRODUCTION_KEYRING_SERVICE,
    };
    use crate::domain::{
        AgentMemoryEntry, AgentSession, AppEventLog, DocumentHistoryEntry,
        FeishuIntegrationSettings, ImIntegrationSettings, ImProviderSettings, KnowledgeBase,
        KnowledgeBaseMemory, KnowledgeBaseSelection, LlmProviderConfig, Note,
        NoteImageAttachmentInput, RequestAuditLog, WorkspaceDocument, WorkspaceEditorState,
        WorkspaceEditorTab, WorkspaceSnapshot, IM_PROVIDER_FEISHU, MEMORY_CATEGORY_OTHER,
        MEMORY_SOURCE_USER,
    };
    use base64::Engine as _;
    use rusqlite::Connection;
    use serde_json::json;
    use std::collections::HashSet;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /** 构造测试用 Agent 会话，避免排序和迁移测试重复铺开完整结构。 */
    fn test_agent_session(id: &str, created_at: &str) -> AgentSession {
        AgentSession {
            id: id.to_owned(),
            title: "测试会话".to_owned(),
            im_identity: None,
            r#type: "task".to_owned(),
            knowledge_base_ids: vec!["kb-a".to_owned()],
            active_note_id: None,
            pinned_note_ids: Vec::new(),
            messages: Vec::new(),
            pending_change: None,
            pending_change_set: None,
            pending_execution: None,
            security_level: "basic".to_owned(),
            context_summary: None,
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
            deleted_at: None,
            model_provider_id: None,
            model_id: None,
            context_usage: None,
        }
    }

    /** 构造最小工作区快照，供编辑器会话过滤规则测试复用。 */
    fn test_workspace_snapshot() -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            knowledge_bases: vec![KnowledgeBase {
                id: "kb-a".to_owned(),
                name: "测试知识库".to_owned(),
                path: "/redacted".to_owned(),
                description: String::new(),
                status: "ready".to_owned(),
                note_count: 1,
                document_count: 1,
                updated_at: "2026/01/01 00:00".to_owned(),
                is_default: true,
                semantic_index_enabled: false,
                scan_report: None,
            }],
            folders: Vec::new(),
            notes: vec![Note {
                id: "note-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                title: "笔记".to_owned(),
                path: "note.md".to_owned(),
                content: String::new(),
                tags: Vec::new(),
                updated_at: "2026/01/01 00:00".to_owned(),
                backlinks: Vec::new(),
                content_hash: String::new(),
            }],
            documents: vec![WorkspaceDocument {
                id: "document-a".to_owned(),
                knowledge_base_id: "kb-a".to_owned(),
                title: "文档".to_owned(),
                path: "document.txt".to_owned(),
                file_type: "txt".to_owned(),
                updated_at: "2026/01/01 00:00".to_owned(),
                content_hash: String::new(),
                content: Some(String::new()),
                preview_available: true,
            }],
            sessions: Vec::new(),
            active_knowledge_base_id: "kb-a".to_owned(),
            active_note_id: String::new(),
            active_document_id: String::new(),
            active_session_id: String::new(),
        }
    }

    #[test]
    fn normalize_workspace_editor_state_filters_invalid_tabs_without_first_file_fallback() {
        let snapshot = test_workspace_snapshot();
        let state = WorkspaceEditorState {
            active_knowledge_base_id: "removed-kb".to_owned(),
            open_tabs: vec![
                WorkspaceEditorTab {
                    kind: "note".to_owned(),
                    id: "note-a".to_owned(),
                },
                WorkspaceEditorTab {
                    kind: "document".to_owned(),
                    id: "missing".to_owned(),
                },
                WorkspaceEditorTab {
                    kind: "unknown".to_owned(),
                    id: "note-a".to_owned(),
                },
            ],
            active_tab: Some(WorkspaceEditorTab {
                kind: "document".to_owned(),
                id: "missing".to_owned(),
            }),
            updated_at: "2026/01/01 00:00".to_owned(),
        };

        let normalized = normalize_workspace_editor_state(&snapshot, state);

        assert_eq!(normalized.active_knowledge_base_id, "kb-a");
        assert_eq!(
            normalized.open_tabs,
            vec![WorkspaceEditorTab {
                kind: "note".to_owned(),
                id: "note-a".to_owned()
            }]
        );
        assert_eq!(normalized.active_tab, None);
    }

    /** 构造测试用 Agent 会话并显式指定更新时间，便于排序测试覆盖 updated_at 路径。 */
    fn test_agent_session_with_updated_at(
        id: &str,
        created_at: &str,
        updated_at: &str,
    ) -> AgentSession {
        let mut session = test_agent_session(id, created_at);
        session.updated_at = updated_at.to_owned();
        session
    }

    /** 旧版 `{ feishu: ... }` IM 设置必须迁移为 providers，避免升级后丢失飞书配置。 */
    #[test]
    fn legacy_im_settings_payload_migrates_to_provider_shape() {
        let payload = r#"{
            "feishu": {
                "enabled": true,
                "domain": "lark",
                "appId": " cli_x ",
                "secretKeyReference": "legacy-secret",
                "defaultKnowledgeBaseIds": ["kb-a"],
                "allowedUserOpenIds": ["ou_user"],
                "allowedChatIds": ["oc_group"],
                "discoveredUserOpenIds": [],
                "discoveredChatIds": [],
                "requireMention": true,
                "updatedAt": "2026-07-06 10:00"
            }
        }"#;
        let mut settings = parse_im_settings_payload(payload).unwrap();

        normalize_im_settings(&mut settings);

        assert_eq!(settings.providers.len(), 1);
        let feishu = settings.providers[0].to_feishu_settings().unwrap();

        assert_eq!(settings.providers[0].provider_id, IM_PROVIDER_FEISHU);
        assert!(feishu.enabled);
        assert_eq!(feishu.domain, "lark");
        assert_eq!(feishu.app_id, "cli_x");
        assert_eq!(
            feishu.secret_key_reference,
            super::FEISHU_SECRET_KEY_REFERENCE
        );
    }

    /** provider 归一化必须去重白名单并从 discovered 中移除已授权对象。 */
    #[test]
    fn im_provider_normalization_deduplicates_and_filters_discovered_peers() {
        let mut settings = super::default_im_settings();
        let provider = settings
            .providers
            .iter_mut()
            .find(|provider| provider.provider_id == IM_PROVIDER_FEISHU)
            .unwrap();

        provider.default_knowledge_base_ids = vec![" kb-a ".to_owned(), "kb-a".to_owned()];
        provider.allowed_user_open_ids = vec![" ou_user ".to_owned(), "ou_user".to_owned()];
        provider.allowed_chat_ids = vec![" oc_group ".to_owned(), "oc_group".to_owned()];
        provider.discovered_user_open_ids = vec!["ou_user".to_owned(), "ou_other".to_owned()];
        provider.discovered_chat_ids = vec!["oc_group".to_owned(), "oc_other".to_owned()];

        normalize_im_settings(&mut settings);
        let provider = settings
            .providers
            .iter()
            .find(|provider| provider.provider_id == IM_PROVIDER_FEISHU)
            .unwrap();

        assert_eq!(provider.default_knowledge_base_ids, vec!["kb-a".to_owned()]);
        assert_eq!(provider.allowed_user_open_ids, vec!["ou_user".to_owned()]);
        assert_eq!(provider.allowed_chat_ids, vec!["oc_group".to_owned()]);
        assert_eq!(
            provider.discovered_user_open_ids,
            vec!["ou_other".to_owned()]
        );
        assert_eq!(provider.discovered_chat_ids, vec!["oc_other".to_owned()]);
    }

    /** 重复 provider 必须合并成一个已启用配置，避免运行态读取到默认禁用副本。 */
    #[test]
    fn im_provider_normalization_merges_duplicate_feishu_providers() {
        let mut settings = ImIntegrationSettings {
            providers: vec![
                ImProviderSettings::from_feishu(FeishuIntegrationSettings {
                    enabled: false,
                    domain: "feishu".to_owned(),
                    app_id: String::new(),
                    secret_key_reference: "legacy-secret".to_owned(),
                    default_knowledge_base_ids: Vec::new(),
                    allowed_user_open_ids: Vec::new(),
                    allowed_chat_ids: Vec::new(),
                    discovered_user_open_ids: Vec::new(),
                    discovered_chat_ids: Vec::new(),
                    require_mention: true,
                    updated_at: "刚刚".to_owned(),
                }),
                ImProviderSettings::from_feishu(FeishuIntegrationSettings {
                    enabled: true,
                    domain: "lark".to_owned(),
                    app_id: "cli_x".to_owned(),
                    secret_key_reference: "legacy-secret".to_owned(),
                    default_knowledge_base_ids: vec!["kb-a".to_owned()],
                    allowed_user_open_ids: vec!["ou_user".to_owned()],
                    allowed_chat_ids: vec!["oc_group".to_owned()],
                    discovered_user_open_ids: vec!["ou_candidate".to_owned()],
                    discovered_chat_ids: vec!["oc_candidate".to_owned()],
                    require_mention: true,
                    updated_at: "2026-07-06 11:00".to_owned(),
                }),
            ],
        };

        normalize_im_settings(&mut settings);

        assert_eq!(settings.providers.len(), 1);
        let feishu = settings.providers[0].to_feishu_settings().unwrap();
        assert!(feishu.enabled);
        assert_eq!(feishu.domain, "lark");
        assert_eq!(feishu.app_id, "cli_x");
        assert_eq!(feishu.default_knowledge_base_ids, vec!["kb-a".to_owned()]);
        assert_eq!(feishu.allowed_user_open_ids, vec!["ou_user".to_owned()]);
        assert_eq!(feishu.allowed_chat_ids, vec!["oc_group".to_owned()]);
        assert_eq!(
            feishu.discovered_user_open_ids,
            vec!["ou_candidate".to_owned()]
        );
        assert_eq!(feishu.discovered_chat_ids, vec!["oc_candidate".to_owned()]);
    }

    /** 写入最小 DOCX zip fixture，只包含预览命令首版会解析的 word/document.xml。 */
    fn write_minimal_docx(path: &Path, document_xml: &str) {
        let file = fs::File::create(path).unwrap();
        let options = SimpleFileOptions::default();
        let mut archive = ZipWriter::new(file);

        archive.start_file("[Content_Types].xml", options).unwrap();
        archive
            .write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
            )
            .unwrap();
        archive.start_file("word/document.xml", options).unwrap();
        archive.write_all(document_xml.as_bytes()).unwrap();
        archive.finish().unwrap();
    }

    /** 构造预览测试所需的普通文档元数据，避免每个用例重复无关字段。 */
    fn test_workspace_document(file_type: &str, path: &str) -> WorkspaceDocument {
        WorkspaceDocument {
            id: format!("document-{file_type}"),
            knowledge_base_id: "kb-test".to_owned(),
            title: "测试文档".to_owned(),
            path: path.to_owned(),
            file_type: file_type.to_owned(),
            updated_at: "刚刚".to_owned(),
            content_hash: String::new(),
            content: None,
            preview_available: file_type != "txt",
        }
    }

    /** 构造最小 PNG 字节，格式识别只依赖文件头即可覆盖附件保存逻辑。 */
    fn test_png_bytes(label: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

        bytes.extend_from_slice(label);
        bytes
    }

    /** 构造图片附件命令入参，避免测试把 base64 编码细节散落到各个用例。 */
    fn test_image_attachment(mime_type: &str, bytes: &[u8]) -> NoteImageAttachmentInput {
        NoteImageAttachmentInput {
            mime_type: mime_type.to_owned(),
            bytes_base64: BASE64_STANDARD.encode(bytes),
            original_file_name: None,
        }
    }

    /** 构造只包含应用事件日志表的内存 SQLite 连接，隔离持久化测试。 */
    fn test_event_log_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();

        connection
            .execute_batch(
                r#"
                CREATE TABLE app_event_logs (
                  id TEXT PRIMARY KEY,
                  level TEXT NOT NULL,
                  category TEXT NOT NULL,
                  event TEXT NOT NULL,
                  message TEXT NOT NULL,
                  status TEXT NOT NULL,
                  operation_id TEXT,
                  session_id TEXT,
                  knowledge_base_id TEXT,
                  entity_type TEXT,
                  entity_id TEXT,
                  relative_path TEXT,
                  duration_ms INTEGER,
                  metadata_json TEXT,
                  created_at TEXT NOT NULL
                );
                "#,
            )
            .unwrap();

        connection
    }

    /** 构造测试事件日志，保留必要字段，避免用例重复冗长结构体。 */
    fn test_app_event_log(id: &str, level: &str, category: &str, created_at: &str) -> AppEventLog {
        AppEventLog {
            id: id.to_owned(),
            level: level.to_owned(),
            category: category.to_owned(),
            event: "test_event".to_owned(),
            message: "测试事件".to_owned(),
            status: "completed".to_owned(),
            operation_id: Some("op-test".to_owned()),
            session_id: None,
            knowledge_base_id: Some("kb-test".to_owned()),
            entity_type: None,
            entity_id: None,
            relative_path: Some("folder/note.md".to_owned()),
            duration_ms: Some(12),
            metadata_json: Some(r#"{"count":1}"#.to_owned()),
            created_at: created_at.to_owned(),
        }
    }

    /** 构造文档历史元数据；正文由调用方另行写入快照文件。 */
    fn test_document_history_entry(
        id: &str,
        target_id: &str,
        content: &str,
        created_at: &str,
    ) -> DocumentHistoryEntry {
        DocumentHistoryEntry {
            id: id.to_owned(),
            target_kind: "note".to_owned(),
            knowledge_base_id: "kb-test".to_owned(),
            target_id: target_id.to_owned(),
            relative_path: "folder/note.md".to_owned(),
            title: "测试笔记".to_owned(),
            file_type: "markdown".to_owned(),
            content_hash: hash_content(content),
            byte_size: content.as_bytes().len(),
            line_count: if content.is_empty() {
                0
            } else {
                content.split('\n').count()
            },
            source: "manual-save".to_owned(),
            session_id: None,
            change_id: None,
            operation_id: None,
            created_at: created_at.to_owned(),
        }
    }

    /** hash 内容变化时必须变化，用于写入冲突检测。 */
    #[test]
    fn hash_changes_when_content_changes() {
        assert_ne!(hash_content("a"), hash_content("b"));
    }

    /** keyring 后端的缺失条目错误应被识别为未配置，而不是模型读取故障。 */
    #[test]
    fn keyring_missing_entry_errors_are_detected() {
        assert!(is_missing_keyring_entry_error(
            "No matching entry found in secure storage"
        ));
        assert!(is_missing_keyring_entry_error(
            "The specified item could not be found in the keychain"
        ));
        assert!(!is_missing_keyring_entry_error(
            "User interaction is not allowed"
        ));
    }

    /** macOS 调试构建必须使用开发 service，其他构建保持正式 service，防止凭据串用。 */
    #[test]
    fn keyring_service_isolated_between_development_and_production_builds() {
        assert_eq!(keyring_service_for_build(true, true), "Orange Dev");
        assert_eq!(keyring_service_for_build(true, false), "Orange");
        assert_eq!(keyring_service_for_build(false, true), "Orange");
    }

    /**
     * 回归测试：重启会清空进程缓存，开发态必须先读取 Orange Dev，而不是提前返回未配置。
     *
     * 使用注入 reader 记录查询顺序，不访问真实 Keychain，也不使用任何用户凭据。
     */
    #[test]
    fn development_cache_miss_reads_current_keychain_service_before_isolation() {
        let key_reference = "orange-test-provider-api-key";
        let mut attempts = Vec::new();
        let lookup = load_keyring_password_after_cache_miss_with(
            DEVELOPMENT_KEYRING_SERVICE,
            key_reference,
            |service, account| {
                attempts.push((service.to_owned(), account.to_owned()));

                if service == DEVELOPMENT_KEYRING_SERVICE && account == key_reference {
                    return Ok(Some("test-only-key".to_owned()));
                }

                Ok(None)
            },
        )
        .unwrap();

        assert_eq!(lookup.api_key, Some("test-only-key".to_owned()));
        assert!(!lookup.requires_migration);
        assert_eq!(
            attempts,
            vec![(
                DEVELOPMENT_KEYRING_SERVICE.to_owned(),
                key_reference.to_owned(),
            ),]
        );
    }

    /** 开发态自身 service 未命中时不得读取 Orange 或 Cici Note，防止调试签名接触生产凭据。 */
    #[test]
    fn development_cache_miss_does_not_fallback_to_production_or_legacy_keychain() {
        let key_reference = "orange-test-provider-api-key";
        let mut attempts = Vec::new();
        let lookup = load_keyring_password_after_cache_miss_with(
            DEVELOPMENT_KEYRING_SERVICE,
            key_reference,
            |service, account| {
                attempts.push((service.to_owned(), account.to_owned()));
                Ok(None)
            },
        )
        .unwrap();

        assert_eq!(lookup.api_key, None);
        assert!(!lookup.requires_migration);
        assert_eq!(
            attempts,
            vec![(
                DEVELOPMENT_KEYRING_SERVICE.to_owned(),
                key_reference.to_owned(),
            ),]
        );
    }

    /** 正式构建读取自身 service 失败后仍需保留历史 Cici Note 凭据迁移能力。 */
    #[test]
    fn production_cache_miss_falls_back_to_legacy_keychain_for_migration() {
        let key_reference = "orange-test-provider-api-key";
        let legacy_key_reference = "cici-note-test-provider-api-key";
        let mut attempts = Vec::new();
        let lookup = load_keyring_password_after_cache_miss_with(
            PRODUCTION_KEYRING_SERVICE,
            key_reference,
            |service, account| {
                attempts.push((service.to_owned(), account.to_owned()));

                if service == "Cici Note" && account == legacy_key_reference {
                    return Ok(Some("test-only-legacy-key".to_owned()));
                }

                Ok(None)
            },
        )
        .unwrap();

        assert_eq!(lookup.api_key, Some("test-only-legacy-key".to_owned()));
        assert!(lookup.requires_migration);
        assert_eq!(
            attempts,
            vec![
                (
                    PRODUCTION_KEYRING_SERVICE.to_owned(),
                    key_reference.to_owned()
                ),
                ("Cici Note".to_owned(), key_reference.to_owned()),
                ("Cici Note".to_owned(), legacy_key_reference.to_owned()),
            ]
        );
    }

    /** keyring 默认后端必须是系统级持久化存储，防止 API key 重启后丢失。 */
    #[test]
    fn model_keyring_uses_persistent_backend() {
        assert!(model_keyring_persists_until_delete());
        assert!(ensure_persistent_model_keyring().is_ok());
    }

    /** 读回校验后的密钥会按 key 引用进入进程缓存，供同一桌面会话内的 Agent turn 复用。 */
    #[test]
    fn model_api_key_cache_round_trips_inside_process() {
        store_model_api_key_in_cache("test-key-reference", "test-key-from-cache").unwrap();

        assert_eq!(
            load_model_api_key_from_cache("test-key-reference").unwrap(),
            Some("test-key-from-cache".to_owned())
        );
    }

    /** 构造 reveal 测试用的 provider；key_reference 与保存命令使用同一派生规则。 */
    fn test_reveal_provider(id: &str) -> LlmProviderConfig {
        LlmProviderConfig {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            provider: "openai-compatible".to_owned(),
            api_base: "https://llm.example/v1".to_owned(),
            model: "test-model".to_owned(),
            key_reference: crate::model_provider::key_reference_for_provider(id),
            enabled: true,
            supports_tools: true,
            requires_api_key: true,
            models: Vec::new(),
            models_fetched_at: None,
            created_at: "刚刚".to_owned(),
            updated_at: "刚刚".to_owned(),
        }
    }

    /** 未出现在当前设置中的 provider 不得按任意 ID 读取 keyring。 */
    #[test]
    fn reveal_model_api_key_rejects_unknown_provider() {
        let providers = vec![test_reveal_provider("provider-known")];
        let error = reveal_model_api_key(&providers, "provider-unknown").unwrap_err();

        assert_eq!(error, "找不到指定的模型 Provider。");
        assert!(!error.contains("sk-"));
    }

    /** 空 providerId 在查找设置前就被拒绝，避免无意义的 keyring 访问。 */
    #[test]
    fn reveal_model_api_key_rejects_blank_provider_id() {
        let providers = vec![test_reveal_provider("provider-known")];
        let error = reveal_model_api_key(&providers, "   ").unwrap_err();

        assert_eq!(error, "模型 Provider ID 不能为空。");
    }

    /** 已配置密钥可通过进程缓存按需揭示，错误路径不得回显明文。 */
    #[test]
    fn reveal_model_api_key_returns_cached_secret_for_known_provider() {
        let provider = test_reveal_provider("provider-reveal-cached");
        let api_key = "sk-test-reveal-cached-key";
        store_model_api_key_in_cache(&provider.key_reference, api_key).unwrap();

        let revealed = reveal_model_api_key(&[provider.clone()], &provider.id).unwrap();

        assert_eq!(revealed.provider_id, provider.id);
        assert_eq!(revealed.api_key, api_key);
    }

    /** 设置里残留的占位 key_reference 不能挡住按 providerId 派生的真实条目。 */
    #[test]
    fn reveal_model_api_key_uses_derived_key_reference_not_stored_placeholder() {
        let mut provider = test_reveal_provider("provider-reveal-derived");
        let api_key = "sk-test-reveal-derived-key";
        let derived_reference = crate::model_provider::key_reference_for_provider(&provider.id);
        store_model_api_key_in_cache(&derived_reference, api_key).unwrap();
        provider.key_reference = "placeholder-not-used-for-reveal".to_owned();

        let revealed = reveal_model_api_key(&[provider.clone()], &provider.id).unwrap();

        assert_eq!(revealed.api_key, api_key);
    }

    /** 设置中存在 provider 但 keyring/缓存都没有密钥时，返回未配置而不是内部错误。 */
    #[test]
    fn reveal_model_api_key_reports_missing_secret_without_leaking_details() {
        let provider = test_reveal_provider("provider-reveal-missing-secret");
        let error =
            reveal_model_api_key(&[provider], "provider-reveal-missing-secret").unwrap_err();

        assert_eq!(error, "系统安全存储中尚未找到模型密钥。");
        assert!(!error.contains("sk-"));
        assert!(!error.contains("provider-reveal-missing-secret"));
    }

    /** 旧版会话如果把创建时间保存成“刚刚”，应优先从前端会话 ID 的时间戳恢复。 */
    #[test]
    fn normalize_session_created_at_uses_timestamp_from_frontend_id() {
        let timestamp_millis = 1_700_000_000_000;
        let mut session =
            test_agent_session(&format!("session-task-{timestamp_millis}-abc123"), "刚刚");
        let expected_created_at = format_local_datetime_from_millis(timestamp_millis).unwrap();

        normalize_session_created_at(&mut session);

        assert_eq!(session.created_at, expected_created_at);
    }

    /** 会话历史按“最后使用时间”倒序排列，最近发过消息的会话排在前面。 */
    #[test]
    fn sort_sessions_by_updated_at_desc_orders_by_updated_at() {
        // 三个会话创建时间相同，仅 updated_at 不同：最新更新者应排到最前面。
        let mut sessions = vec![
            test_agent_session_with_updated_at(
                "session-a-1700000000000",
                "2023/11/14 22:13",
                "2023/11/20 09:00",
            ),
            test_agent_session_with_updated_at(
                "session-b-1700000030000",
                "2023/11/14 22:13",
                "2023/11/22 18:30",
            ),
            test_agent_session_with_updated_at(
                "session-c-1699999940000",
                "2023/11/14 22:13",
                "2023/11/15 07:45",
            ),
        ];

        sort_sessions_by_updated_at_desc(&mut sessions);

        let session_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            session_ids,
            vec![
                "session-b-1700000030000",
                "session-a-1700000000000",
                "session-c-1699999940000",
            ]
        );
    }

    /** updated_at 仍是“刚刚”占位值时，应退回会话 ID 里的毫秒时间戳排序，保持稳定兜底。 */
    #[test]
    fn sort_sessions_by_updated_at_desc_falls_back_to_id_timestamp() {
        let mut sessions = vec![
            test_agent_session_with_updated_at(
                "session-task-1700000000000-old",
                "2023/11/14 22:13",
                "刚刚",
            ),
            test_agent_session_with_updated_at(
                "session-task-1700000030000-new",
                "2023/11/14 22:13",
                "刚刚",
            ),
            test_agent_session_with_updated_at(
                "session-task-1699999940000-earliest",
                "2023/11/14 22:12",
                "刚刚",
            ),
        ];

        sort_sessions_by_updated_at_desc(&mut sessions);

        let session_ids = sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            session_ids,
            vec![
                "session-task-1700000030000-new",
                "session-task-1700000000000-old",
                "session-task-1699999940000-earliest",
            ]
        );
    }

    /** 旧版审计日志如果保存成“刚刚”，读取或写入前应改成具体本地时间。 */
    #[test]
    fn normalize_audit_log_created_at_replaces_placeholder() {
        let mut log = RequestAuditLog {
            id: "audit-a".to_owned(),
            kind: "model_turn".to_owned(),
            session_id: Some("session-a".to_owned()),
            scope_summary: "测试知识库".to_owned(),
            content_summary: "模型请求".to_owned(),
            tool_summary: "model_request".to_owned(),
            created_at: "刚刚".to_owned(),
        };

        normalize_audit_log_created_at(&mut log);

        assert_ne!(log.created_at, "刚刚");
        assert!(!log.created_at.trim().is_empty());
    }

    /** 应用事件日志读取应按最新写入倒序，并受 limit 限制。 */
    #[test]
    fn app_event_logs_query_descending_with_limit() {
        let connection = test_event_log_connection();

        insert_app_event_log(
            &connection,
            &test_app_event_log("event-old", "info", "editor", "2026/06/23 10:00"),
        )
        .unwrap();
        insert_app_event_log(
            &connection,
            &test_app_event_log("event-new", "error", "agent", "2026/06/23 10:01"),
        )
        .unwrap();

        let logs = query_app_event_logs(&connection, 1, None, None).unwrap();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "event-new");
    }

    /** 应用事件日志筛选应同时支持级别和分类，且不会返回其他类别记录。 */
    #[test]
    fn app_event_logs_query_filters_level_and_category() {
        let connection = test_event_log_connection();

        insert_app_event_log(
            &connection,
            &test_app_event_log("event-agent-error", "error", "agent", "2026/06/23 10:00"),
        )
        .unwrap();
        insert_app_event_log(
            &connection,
            &test_app_event_log("event-editor-error", "error", "editor", "2026/06/23 10:01"),
        )
        .unwrap();
        insert_app_event_log(
            &connection,
            &test_app_event_log("event-agent-info", "info", "agent", "2026/06/23 10:02"),
        )
        .unwrap();

        let logs = query_app_event_logs(&connection, 10, Some("error"), Some("agent")).unwrap();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "event-agent-error");
    }

    /** 应用事件日志保留策略应移除过期记录，避免本地数据库无限增长。 */
    #[test]
    fn app_event_logs_prune_removes_expired_records() {
        let connection = test_event_log_connection();

        insert_app_event_log(
            &connection,
            &test_app_event_log("event-expired", "info", "app", "2000/01/01 00:00"),
        )
        .unwrap();
        insert_app_event_log(
            &connection,
            &test_app_event_log("event-current", "info", "app", &format_local_datetime()),
        )
        .unwrap();

        prune_app_event_logs(&connection).unwrap();

        let logs = query_app_event_logs(&connection, 10, None, None).unwrap();

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, "event-current");
    }

    /** 文档历史 schema 应幂等创建，避免旧数据库升级时缺表。 */
    #[test]
    fn document_history_schema_is_created_idempotently() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("history-schema.sqlite3");
        let connection = Connection::open(&database_path).unwrap();

        ensure_database_schema(&connection, &database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'document_history_entries'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(table_count, 1);
    }

    /** 历史快照读取必须拒绝路径穿越和缺失文件。 */
    #[test]
    fn document_history_snapshot_path_is_validated() {
        let dir = tempdir().unwrap();

        write_document_history_snapshot(dir.path(), "history-safe", "第一版").unwrap();

        assert_eq!(
            read_document_history_snapshot(dir.path(), "history-safe").unwrap(),
            "第一版"
        );
        assert!(read_document_history_snapshot(dir.path(), "../history-safe").is_err());
        assert!(read_document_history_snapshot(dir.path(), "history-missing").is_err());
    }

    /** 文档历史应跳过最新相同 hash，并按数量保留策略删除旧快照。 */
    #[test]
    fn document_history_dedupes_latest_hash_and_prunes_snapshots() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("history-prune.sqlite3");
        let connection = Connection::open(&database_path).unwrap();

        ensure_database_schema(&connection, &database_path).unwrap();

        for index in 0..=MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE {
            let content = format!("版本 {index}");
            let entry = test_document_history_entry(
                &format!("history-prune-{index}"),
                "note-a",
                &content,
                &format_local_datetime(),
            );

            write_document_history_snapshot(dir.path(), &entry.id, &content).unwrap();
            insert_document_history_entry(&connection, &entry).unwrap();
        }

        assert_eq!(
            load_latest_document_history_hash(&connection, "note", "note-a").unwrap(),
            Some(hash_content(&format!(
                "版本 {}",
                MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE
            )))
        );

        let summary =
            prune_document_history_entries(&connection, dir.path(), "note", "note-a").unwrap();
        let remaining_ids =
            load_document_history_ids_for_target(&connection, "note", "note-a").unwrap();

        assert_eq!(summary.removed_count, 1);
        assert_eq!(summary.cleanup_failure_count, 0);
        assert_eq!(remaining_ids.len(), MAX_DOCUMENT_HISTORY_ENTRIES_PER_FILE);
        assert!(read_document_history_snapshot(dir.path(), "history-prune-0").is_err());
    }

    /** 路径穿越必须被阻止，防止 Agent 写出知识库根目录。 */
    #[test]
    fn reject_path_outside_root() {
        let dir = tempdir().unwrap();
        let result = resolve_inside_root(dir.path(), "../outside.md");

        assert!(result.is_err());
    }

    /** 路径穿越被拒绝时不应提前创建知识库外部目录。 */
    #[test]
    fn reject_path_outside_root_without_creating_parent() {
        let dir = tempdir().unwrap();
        let outside_name = format!("orange-outside-parent-{}", create_id("test"));
        let outside_parent = dir.path().parent().unwrap().join(&outside_name);
        let result = resolve_inside_root(dir.path(), &format!("../{outside_name}/outside.md"));

        assert!(result.is_err());
        assert!(!outside_parent.exists());
    }

    /** 原子写入应在目标路径生成完整文件。 */
    #[test]
    fn atomic_write_creates_markdown_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");

        atomic_write_markdown(&path, "# Title").unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "# Title");
    }

    /** 保存已有笔记的路径解析不应创建缺失父目录。 */
    #[test]
    fn existing_file_resolver_does_not_create_missing_parent() {
        let dir = tempdir().unwrap();
        let result = resolve_existing_file_inside_root(dir.path(), "missing/note.md");

        assert!(result.is_err());
        assert!(!dir.path().join("missing").exists());
    }

    /** 新建 Markdown 文档应生成唯一文件名，不覆盖已有未命名文件。 */
    #[test]
    fn create_blank_markdown_file_uses_unique_path() {
        let dir = tempdir().unwrap();
        let first_path = create_blank_markdown_file(dir.path(), "", None).unwrap();
        let second_path = create_blank_markdown_file(dir.path(), "", None).unwrap();

        assert_eq!(first_path, "未命名.md");
        assert_eq!(second_path, "未命名 2.md");
        assert_eq!(fs::read_to_string(dir.path().join(first_path)).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.path().join(second_path)).unwrap(),
            ""
        );
    }

    /** 根目录新建文档允许省略扩展名，并默认补齐 .md。 */
    #[test]
    fn create_markdown_file_in_root_appends_default_extension() {
        let dir = tempdir().unwrap();
        let relative_path = create_blank_markdown_file(dir.path(), "", Some("Root Note")).unwrap();

        assert_eq!(relative_path, "Root Note.md");
        assert_eq!(
            fs::read_to_string(dir.path().join("Root Note.md")).unwrap(),
            ""
        );
    }

    /** 子目录新建文档必须落在用户点击的目录下，不再由当前笔记上下文推断。 */
    #[test]
    fn create_markdown_file_in_child_directory() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Child")).unwrap();

        let relative_path =
            create_blank_markdown_file(dir.path(), "Child", Some("Nested.md")).unwrap();

        assert_eq!(relative_path, "Child/Nested.md");
        assert!(dir.path().join("Child").join("Nested.md").exists());
    }

    /** 新建文档应拒绝路径穿越、重复名称和非 Markdown 扩展名。 */
    #[test]
    fn create_markdown_file_rejects_invalid_or_existing_targets() {
        let dir = tempdir().unwrap();

        fs::write(dir.path().join("taken.md"), "# Taken").unwrap();

        assert!(validate_new_markdown_file_name("../x.md").is_err());
        assert!(validate_new_markdown_file_name("").is_err());
        assert!(validate_new_markdown_file_name("note.txt").is_err());
        assert!(create_blank_markdown_file(dir.path(), "", Some("taken.md")).is_err());
        assert!(create_blank_markdown_file(dir.path(), "../outside", Some("x.md")).is_err());
    }

    /** 新建 TXT 文档应支持默认名、省略扩展名，并拒绝路径穿越、重复名称和非 txt 扩展名。 */
    #[test]
    fn create_text_document_validates_file_name_and_target() {
        let dir = tempdir().unwrap();
        let first_path = create_blank_text_document_file(dir.path(), "", None).unwrap();
        let named_path = create_blank_text_document_file(dir.path(), "", Some("Draft")).unwrap();

        assert_eq!(first_path, "未命名.txt");
        assert_eq!(named_path, "Draft.txt");
        assert_eq!(
            fs::read_to_string(dir.path().join("Draft.txt")).unwrap(),
            ""
        );
        assert!(validate_new_text_document_file_name("../x.txt").is_err());
        assert!(validate_new_text_document_file_name("").is_err());
        assert!(validate_new_text_document_file_name("note.md").is_err());
        assert!(create_blank_text_document_file(dir.path(), "", Some("Draft.txt")).is_err());
        assert!(create_blank_text_document_file(dir.path(), "../outside", Some("x.txt")).is_err());
    }

    /** 根目录新建文件夹成功后返回相对于知识库根目录的路径。 */
    #[test]
    fn create_folder_in_root_directory() {
        let dir = tempdir().unwrap();
        let relative_path = create_folder(dir.path(), "", "New Folder").unwrap();

        assert_eq!(relative_path, "New Folder");
        assert!(dir.path().join("New Folder").is_dir());
    }

    /** 子目录新建文件夹只创建单级子目录，并保留父目录结构。 */
    #[test]
    fn create_folder_in_child_directory() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Parent")).unwrap();

        let relative_path = create_folder(dir.path(), "Parent", "Child").unwrap();

        assert_eq!(relative_path, "Parent/Child");
        assert!(dir.path().join("Parent").join("Child").is_dir());
    }

    /** 新建文件夹必须拒绝路径穿越、隐藏目录、扫描忽略目录和重复名称。 */
    #[test]
    fn create_folder_rejects_invalid_or_existing_targets() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("taken")).unwrap();

        assert!(validate_folder_name("../x").is_err());
        assert!(validate_folder_name("").is_err());
        assert!(validate_folder_name(".hidden").is_err());
        assert!(validate_folder_name("node_modules").is_err());
        assert!(create_folder(dir.path(), "", "taken").is_err());
        assert!(create_folder(dir.path(), "../outside", "x").is_err());
    }

    /** 粘贴图片应保存到当前 Markdown 同级 assets/<笔记名>/，并返回可插入的标准 Markdown。 */
    #[test]
    fn save_note_image_attachments_creates_note_assets_folder() {
        let dir = tempdir().unwrap();
        let note_path = dir.path().join("notes").join("My Note.md");
        let png_bytes = test_png_bytes(b"first");

        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "# My Note").unwrap();

        let attachments = save_note_image_attachments(
            dir.path(),
            "notes/My Note.md",
            &[test_image_attachment("image/png", &png_bytes)],
        )
        .unwrap();
        let attachment = &attachments[0];

        assert_eq!(attachments.len(), 1);
        assert!(attachment
            .relative_path
            .starts_with("assets/My Note/pasted-"));
        assert!(attachment.relative_path.ends_with(".png"));
        assert!(attachment
            .markdown
            .starts_with("![image](assets/My%20Note/pasted-"));
        assert!(dir
            .path()
            .join("notes")
            .join(&attachment.relative_path)
            .exists());
        assert_eq!(
            fs::read(dir.path().join("notes").join(&attachment.relative_path)).unwrap(),
            png_bytes
        );
    }

    /** 同一批次粘贴相同图片必须生成不同文件名，不能覆盖已写入附件。 */
    #[test]
    fn save_note_image_attachments_does_not_overwrite_duplicate_images() {
        let dir = tempdir().unwrap();
        let png_bytes = test_png_bytes(b"same-image");

        fs::write(dir.path().join("note.md"), "# Note").unwrap();

        let attachments = save_note_image_attachments(
            dir.path(),
            "note.md",
            &[
                test_image_attachment("image/png", &png_bytes),
                test_image_attachment("image/png", &png_bytes),
            ],
        )
        .unwrap();

        assert_eq!(attachments.len(), 2);
        assert_ne!(attachments[0].relative_path, attachments[1].relative_path);
        assert!(dir.path().join(&attachments[0].relative_path).exists());
        assert!(dir.path().join(&attachments[1].relative_path).exists());
    }

    /** MIME 与文件头不一致时应拒绝保存，防止伪造类型的内容进入知识库。 */
    #[test]
    fn save_note_image_attachments_rejects_mime_mismatch() {
        let dir = tempdir().unwrap();
        let png_bytes = test_png_bytes(b"mismatch");

        fs::write(dir.path().join("note.md"), "# Note").unwrap();

        let result = save_note_image_attachments(
            dir.path(),
            "note.md",
            &[test_image_attachment("image/jpeg", &png_bytes)],
        );

        assert!(result.is_err());
        assert!(!dir.path().join("assets").exists());
    }

    /** 批量粘贴中任一图片非法时不应修改正文，也不应提前创建附件目录。 */
    #[test]
    fn save_note_image_attachments_rejects_batch_without_partial_files() {
        let dir = tempdir().unwrap();
        let png_bytes = test_png_bytes(b"valid");
        let svg_bytes = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;

        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested").join("note.md"), "# Note").unwrap();

        let result = save_note_image_attachments(
            dir.path(),
            "nested/note.md",
            &[
                test_image_attachment("image/png", &png_bytes),
                test_image_attachment("image/svg+xml", svg_bytes),
            ],
        );

        assert!(result.is_err());
        assert!(!dir.path().join("nested").join("assets").exists());
    }

    /** 重命名应拒绝路径穿越、空名和非 Markdown 扩展名。 */
    #[test]
    fn rename_rejects_invalid_file_names() {
        assert!(validate_markdown_file_name("../x.md").is_err());
        assert!(validate_markdown_file_name("").is_err());
        assert!(validate_markdown_file_name("note.txt").is_err());
        assert!(validate_text_document_file_name("../x.txt").is_err());
        assert!(validate_text_document_file_name("").is_err());
        assert!(validate_text_document_file_name("note.md").is_err());
    }

    /** 重命名不能覆盖同目录下已有 Markdown 文件。 */
    #[test]
    fn rename_rejects_existing_target() {
        let dir = tempdir().unwrap();

        fs::write(dir.path().join("old.md"), "# Old").unwrap();
        fs::write(dir.path().join("taken.md"), "# Taken").unwrap();

        let result = rename_markdown_file(dir.path(), "old.md", "taken.md");

        assert!(result.is_err());
        assert!(dir.path().join("old.md").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("taken.md")).unwrap(),
            "# Taken"
        );
    }

    /** 重命名成功后原路径消失，新路径保留原始正文和 hash。 */
    #[test]
    fn rename_preserves_content_and_hash() {
        let dir = tempdir().unwrap();
        let old_path = dir.path().join("old.md");

        fs::write(&old_path, "# Old\n\n正文").unwrap();

        let (next_relative_path, content, content_hash) =
            rename_markdown_file(dir.path(), "old.md", "new.md").unwrap();

        assert_eq!(next_relative_path, "new.md");
        assert_eq!(content, "# Old\n\n正文");
        assert_eq!(content_hash, hash_content("# Old\n\n正文"));
        assert!(!old_path.exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("new.md")).unwrap(),
            "# Old\n\n正文"
        );
    }

    /** TXT 重命名只改文件名，保留正文和 hash，并拒绝覆盖同目录已有文件。 */
    #[test]
    fn rename_text_document_preserves_content_and_rejects_existing_target() {
        let dir = tempdir().unwrap();

        fs::write(dir.path().join("old.txt"), "plain text").unwrap();
        fs::write(dir.path().join("taken.txt"), "taken").unwrap();

        assert!(rename_text_document_file(dir.path(), "old.txt", "taken.txt").is_err());

        let (next_relative_path, content, content_hash) =
            rename_text_document_file(dir.path(), "old.txt", "new.txt").unwrap();

        assert_eq!(next_relative_path, "new.txt");
        assert_eq!(content, "plain text");
        assert_eq!(content_hash, hash_content("plain text"));
        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "plain text"
        );
    }

    /** 删除 hash 不一致时必须拒绝，避免误删外部编辑器刚改过的文件。 */
    #[test]
    fn delete_rejects_hash_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");

        fs::write(&path, "# Changed").unwrap();

        let result = trash_markdown_file(dir.path(), "note.md", &hash_content("# Original"));

        assert!(result.is_err());
        assert!(path.exists());
    }

    /** 删除路径越界必须拒绝。 */
    #[test]
    fn delete_rejects_path_outside_root() {
        let dir = tempdir().unwrap();
        let result = trash_markdown_file(dir.path(), "../outside.md", &hash_content(""));

        assert!(result.is_err());
    }

    /** 删除成功后文件应离开原路径，由系统回收站负责恢复能力。 */
    #[test]
    fn delete_moves_file_out_of_original_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("note.md");
        let content = "# Trash me";

        fs::write(&path, content).unwrap();
        trash_markdown_file_with(
            dir.path(),
            "note.md",
            &hash_content(content),
            |target_path| {
                fs::remove_file(target_path).map_err(|error| format!("测试删除失败：{error}"))
            },
        )
        .unwrap();

        assert!(!path.exists());
    }

    /** TXT 保存走原子写入，删除前继续用 hash 冲突检测保护外部修改。 */
    #[test]
    fn text_document_save_and_delete_use_hash_guard() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("draft.txt");

        atomic_write_text_document(&path, "初稿").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "初稿");
        assert!(trash_text_document_file_with(
            dir.path(),
            "draft.txt",
            &hash_content("旧版本"),
            |target_path| {
                fs::remove_file(target_path).map_err(|error| format!("测试删除失败：{error}"))
            },
        )
        .is_err());
        assert!(path.exists());

        trash_text_document_file_with(
            dir.path(),
            "draft.txt",
            &hash_content("初稿"),
            |target_path| {
                fs::remove_file(target_path).map_err(|error| format!("测试删除失败：{error}"))
            },
        )
        .unwrap();

        assert!(!path.exists());
    }

    /** 稳定笔记 ID 必须只由知识库和路径决定，确保重扫后引用仍可匹配。 */
    #[test]
    fn stable_note_id_uses_knowledge_base_and_path() {
        let first_id = create_stable_note_id("kb-a", "A/Note.md");
        let second_id = create_stable_note_id("kb-a", "A/Note.md");
        let other_knowledge_base_id = create_stable_note_id("kb-b", "A/Note.md");

        assert_eq!(first_id, second_id);
        assert_ne!(first_id, other_knowledge_base_id);
    }

    /** 扫描应跳过大型依赖目录，并把坏 Markdown 文件作为报告错误而不是整库失败。 */
    #[test]
    fn scan_reports_failed_files_and_skipped_directories() {
        let dir = tempdir().unwrap();
        let valid_path = dir.path().join("notes").join("ok.md");
        let invalid_path = dir.path().join("broken.md");
        let skipped_path = dir.path().join("node_modules").join("ignored.md");

        fs::create_dir_all(valid_path.parent().unwrap()).unwrap();
        fs::create_dir_all(skipped_path.parent().unwrap()).unwrap();
        fs::write(&valid_path, "# 可读笔记\n\n正文").unwrap();
        fs::write(&invalid_path, [0xff, 0xfe, 0xfd]).unwrap();
        fs::write(&skipped_path, "# 忽略").unwrap();

        let selection = KnowledgeBaseSelection {
            id: "kb-test".to_owned(),
            name: "测试库".to_owned(),
            path: dir.path().to_string_lossy().to_string(),
            note_count: 0,
        };
        let (knowledge_base, folders, notes) = scan_markdown_directory(&selection).unwrap();
        let report = knowledge_base.scan_report.unwrap();

        assert_eq!(notes.len(), 1);
        assert!(folders.iter().any(|folder| folder.path == "notes"));
        assert_eq!(report.scanned_file_count, 1);
        assert_eq!(report.failed_file_count, 1);
        assert_eq!(report.skipped_directories, vec!["node_modules"]);
        assert!(report.errors[0].contains("broken.md"));
    }

    /** 支持文档扫描应区分 Markdown、TXT、DOCX、PDF、图片，并忽略不支持的文件类型。 */
    #[test]
    fn scan_supported_documents_reports_documents_by_type() {
        let dir = tempdir().unwrap();
        let note_path = dir.path().join("notes").join("ok.md");
        let txt_path = dir.path().join("notes").join("draft.txt");
        let docx_path = dir.path().join("docs").join("brief.docx");
        let pdf_path = dir.path().join("docs").join("spec.pdf");
        let image_path = dir.path().join("assets").join("diagram.png");

        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::create_dir_all(docx_path.parent().unwrap()).unwrap();
        fs::create_dir_all(image_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "# 可读笔记\n\n正文").unwrap();
        fs::write(&txt_path, "纯文本正文").unwrap();
        write_minimal_docx(
            &docx_path,
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>DOCX 正文</w:t></w:r></w:p></w:body></w:document>"#,
        );
        fs::write(&pdf_path, b"%PDF-1.4\n").unwrap();
        fs::write(&image_path, test_png_bytes(b"scan")).unwrap();
        fs::write(dir.path().join("ignored.bin"), b"binary").unwrap();

        let selection = KnowledgeBaseSelection {
            id: "kb-docs".to_owned(),
            name: "多类型测试库".to_owned(),
            path: dir.path().to_string_lossy().to_string(),
            note_count: 0,
        };
        let (knowledge_base, _folders, notes, documents) =
            scan_supported_documents_directory(&selection).unwrap();
        let report = knowledge_base.scan_report.unwrap();

        assert_eq!(notes.len(), 1);
        assert_eq!(documents.len(), 4);
        assert_eq!(knowledge_base.note_count, 1);
        assert_eq!(knowledge_base.document_count, 4);
        assert_eq!(report.scanned_file_count, 5);
        assert_eq!(report.scanned_by_type.get("markdown"), Some(&1));
        assert_eq!(report.scanned_by_type.get("txt"), Some(&1));
        assert_eq!(report.scanned_by_type.get("docx"), Some(&1));
        assert_eq!(report.scanned_by_type.get("pdf"), Some(&1));
        assert_eq!(report.scanned_by_type.get("image"), Some(&1));
        assert_eq!(
            notes[0].updated_at,
            file_modified_local_datetime(&note_path).unwrap()
        );
        assert!(documents.iter().any(|document| document.file_type == "txt"
            && document.content.as_deref() == Some("纯文本正文")
            && document.updated_at == file_modified_local_datetime(&txt_path).unwrap()
            && !document.preview_available));
        assert!(documents.iter().any(|document| document.file_type == "docx"
            && document.updated_at == file_modified_local_datetime(&docx_path).unwrap()
            && document.preview_available));
        assert!(documents.iter().any(|document| document.file_type == "pdf"
            && document.updated_at == file_modified_local_datetime(&pdf_path).unwrap()
            && document.preview_available));
        assert!(documents
            .iter()
            .any(|document| document.file_type == "image"
                && document.updated_at == file_modified_local_datetime(&image_path).unwrap()
                && document.preview_available));
    }

    /** 扫描应返回没有 Markdown 文件的空目录，让前端目录树能显示真实空文件夹。 */
    #[test]
    fn scan_returns_empty_folder_nodes() {
        let dir = tempdir().unwrap();

        fs::create_dir(dir.path().join("Empty")).unwrap();

        let selection = KnowledgeBaseSelection {
            id: "kb-empty".to_owned(),
            name: "空目录测试库".to_owned(),
            path: dir.path().to_string_lossy().to_string(),
            note_count: 0,
        };
        let (_knowledge_base, folders, notes) = scan_markdown_directory(&selection).unwrap();

        assert!(notes.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].path, "Empty");
    }

    /** 根目录 AGENTS.md 已注入 system，快照检索不得再把它当知识命中。 */
    #[test]
    fn search_snapshot_notes_excludes_root_project_instruction() {
        let mut snapshot = test_workspace_snapshot();
        snapshot.notes[0].content = "标签必须使用小写连字符。".to_owned();
        snapshot.notes.push(Note {
            id: "note-agents".to_owned(),
            knowledge_base_id: "kb-a".to_owned(),
            title: "Agent 说明书".to_owned(),
            path: "AGENTS.md".to_owned(),
            content: "标签必须使用小写连字符。这是项目说明书。".to_owned(),
            tags: Vec::new(),
            updated_at: "2026/01/01 00:00".to_owned(),
            backlinks: Vec::new(),
            content_hash: String::new(),
        });
        let selected_ids: HashSet<&str> = ["kb-a"].into_iter().collect();
        let citations = search_snapshot_notes(&snapshot, &selected_ids, "标签");

        assert!(citations.iter().any(|citation| citation.path == "note.md"));
        assert!(citations
            .iter()
            .all(|citation| citation.path != "AGENTS.md"));
    }

    /** DOCX 预览应从最小 fixture 中抽取标题和段落，损坏 zip 应返回可展示错误。 */
    #[test]
    fn docx_preview_extracts_blocks_and_rejects_corrupt_file() {
        let dir = tempdir().unwrap();
        let docx_path = dir.path().join("preview.docx");
        let corrupt_path = dir.path().join("corrupt.docx");

        write_minimal_docx(
            &docx_path,
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>标题</w:t></w:r></w:p><w:p><w:r><w:t>第一段</w:t></w:r></w:p></w:body></w:document>"#,
        );
        fs::write(&corrupt_path, b"not a zip").unwrap();

        let blocks = extract_docx_preview_blocks(&docx_path).unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].r#type, "heading");
        assert_eq!(blocks[0].text, "标题");
        assert_eq!(blocks[1].r#type, "paragraph");
        assert_eq!(blocks[1].text, "第一段");
        assert!(extract_docx_preview_blocks(&corrupt_path).is_err());
    }

    /** DOCX 读取应复用预览解析结果，并保留表格来源及结构块序号。 */
    #[test]
    fn document_text_extraction_reads_docx_blocks() {
        let dir = tempdir().unwrap();
        let docx_path = dir.path().join("brief.docx");
        write_minimal_docx(
            &docx_path,
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>段落</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>单元格</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#,
        );

        let extraction =
            extract_document_text(dir.path(), &test_workspace_document("docx", "brief.docx"))
                .unwrap();

        assert_eq!(extraction.blocks.len(), 2);
        assert_eq!(extraction.blocks[0].text, "段落");
        assert_eq!(extraction.blocks[1].r#type, "table");
        assert_eq!(extraction.blocks[1].text, "单元格");
        assert!(extraction.content_chars > 0);
    }

    /** PDF 预览只允许返回知识库内文件的 asset 路径，拒绝越界相对路径。 */
    #[test]
    fn pdf_preview_returns_asset_path_and_rejects_outside_path() {
        let dir = tempdir().unwrap();
        let pdf_path = dir.path().join("spec.pdf");

        fs::write(&pdf_path, b"%PDF-1.4\n").unwrap();

        let preview =
            load_document_preview(dir.path(), &test_workspace_document("pdf", "spec.pdf")).unwrap();
        let canonical_pdf_path = fs::canonicalize(&pdf_path).unwrap();

        assert_eq!(preview.file_type, "pdf");
        assert_eq!(
            preview.asset_path,
            Some(canonical_pdf_path.to_string_lossy().to_string())
        );
        assert_eq!(preview.content_hash, hash_bytes(b"%PDF-1.4\n"));
        assert!(load_document_preview(
            dir.path(),
            &test_workspace_document("pdf", "../outside.pdf")
        )
        .is_err());
    }

    /** 图片预览返回知识库内文件的 asset 路径，并按二进制内容计算 hash。 */
    #[test]
    fn image_preview_returns_asset_path() {
        let dir = tempdir().unwrap();
        let image_path = dir.path().join("diagram.webp");
        let image_bytes = [b"RIFF".as_slice(), &[0, 0, 0, 0], b"WEBP"].concat();

        fs::write(&image_path, &image_bytes).unwrap();

        let preview = load_document_preview(
            dir.path(),
            &test_workspace_document("image", "diagram.webp"),
        )
        .unwrap();
        let canonical_image_path = fs::canonicalize(&image_path).unwrap();

        assert_eq!(preview.file_type, "image");
        assert_eq!(
            preview.asset_path,
            Some(canonical_image_path.to_string_lossy().to_string())
        );
        assert_eq!(preview.content_hash, hash_bytes(&image_bytes));
    }

    /** 脱敏应拦截手机号、API key 前缀、bearer token 和身份证号。 */
    #[test]
    fn redact_memory_secrets_intercepts_pii_and_tokens() {
        let phone_redacted = redact_memory_secrets("联系我 13800138000，备用 手机号：13900139000");
        assert!(phone_redacted.contains("[已脱敏]"));
        assert!(!phone_redacted.contains("13800138000"));
        assert!(!phone_redacted.contains("13900139000"));

        let api_key_redacted = redact_memory_secrets("我的 key 是 sk-abcd1234efgh5678");
        assert!(api_key_redacted.contains("[已脱敏]"));
        assert!(!api_key_redacted.contains("sk-abcd1234efgh5678"));

        let bearer_redacted = redact_memory_secrets("Authorization: Bearer abcdef123456");
        assert!(bearer_redacted.contains("[已脱敏]"));
        assert!(!bearer_redacted.contains("Bearer"));
        assert!(!bearer_redacted.contains("abcdef123456"));

        let id_card_redacted = redact_memory_secrets("身份证：110101199003071234");
        assert!(id_card_redacted.contains("[已脱敏]"));
        assert!(!id_card_redacted.contains("110101199003071234"));

        let api_assignment_redacted = redact_memory_secrets("api_key=ak_live_12345678");
        assert!(api_assignment_redacted.contains("[已脱敏]"));
        assert!(!api_assignment_redacted.contains("ak_live_12345678"));

        let password_redacted = redact_memory_secrets("密码：abc123456");
        assert!(password_redacted.contains("[已脱敏]"));
        assert!(!password_redacted.contains("abc123456"));

        // 普通偏好文本不应被误伤。
        let clean = redact_memory_secrets("标签统一使用小写连字符");
        assert_eq!(clean, "标签统一使用小写连字符");
    }

    /** 记忆归一化应修正未知分类和来源，并继续对正文脱敏。 */
    #[test]
    fn normalize_knowledge_base_memory_sanitizes_category_source_and_content() {
        let mut memory = KnowledgeBaseMemory {
            knowledge_base_id: "wrong-kb".to_owned(),
            enabled: true,
            entries: vec![AgentMemoryEntry {
                id: "mem-1".to_owned(),
                category: "unknownCategory".to_owned(),
                content: "密码：abc123456，标签用短横线".to_owned(),
                source: "external".to_owned(),
                created_at: String::new(),
                updated_at: String::new(),
            }],
            updated_at: String::new(),
        };

        let redacted_hits = normalize_knowledge_base_memory(&mut memory, "kb-a");

        assert_eq!(redacted_hits, 1);
        assert_eq!(memory.knowledge_base_id, "kb-a");
        assert_eq!(memory.entries[0].category, MEMORY_CATEGORY_OTHER);
        assert_eq!(memory.entries[0].source, MEMORY_SOURCE_USER);
        assert!(memory.entries[0].content.contains("[已脱敏]"));
        assert!(!memory.entries[0].content.contains("abc123456"));
        assert!(!memory.updated_at.is_empty());
        assert!(!memory.entries[0].updated_at.is_empty());
    }

    /** 模型 transcript 独立落库，不能写进会话 payload_json 以免撑爆前端快照。 */
    #[test]
    fn agent_session_transcript_round_trips_independently_of_session_payload() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("transcript.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let messages = vec![
            json!({ "role": "user", "content": "完整工具结果应保留" }),
            json!({
                "role": "tool",
                "tool_call_id": "call_abc123",
                "content": "{\"matches\":[{\"title\":\"完整命中\"}]}"
            }),
        ];
        persist_agent_session_transcript(&connection, "session-a", &messages).unwrap();

        let loaded =
            load_agent_session_transcript_from_connection(&connection, "session-a").unwrap();
        assert_eq!(loaded, Some(messages));

        let mut snapshot = test_workspace_snapshot();
        snapshot
            .sessions
            .push(test_agent_session("session-a", "2026/01/01 00:00"));
        let transaction = connection.transaction().unwrap();
        persist_sessions_in_transaction(&transaction, &snapshot).unwrap();
        transaction.commit().unwrap();

        let payload_json: String = connection
            .query_row(
                "SELECT payload_json FROM agent_sessions WHERE id = ?1",
                ["session-a"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!payload_json.contains("完整工具结果应保留"));
        assert!(!payload_json.contains("call_abc123"));
        let loaded =
            load_agent_session_transcript_from_connection(&connection, "session-a").unwrap();
        assert!(loaded.unwrap()[0]["content"]
            .as_str()
            .unwrap()
            .contains("完整工具结果应保留"));
    }

    /** 会话从快照中消失后，对应 transcript 应被清理。 */
    #[test]
    fn persist_sessions_removes_orphan_transcripts() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("transcript-orphan.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        persist_agent_session_transcript(
            &connection,
            "session-keep",
            &[json!({ "role": "user", "content": "保留" })],
        )
        .unwrap();
        persist_agent_session_transcript(
            &connection,
            "session-drop",
            &[json!({ "role": "user", "content": "删除" })],
        )
        .unwrap();

        let mut snapshot = test_workspace_snapshot();
        snapshot
            .sessions
            .push(test_agent_session("session-keep", "2026/01/01 00:00"));
        let transaction = connection.transaction().unwrap();
        persist_sessions_in_transaction(&transaction, &snapshot).unwrap();
        transaction.commit().unwrap();

        assert!(
            load_agent_session_transcript_from_connection(&connection, "session-keep")
                .unwrap()
                .is_some()
        );
        assert!(
            load_agent_session_transcript_from_connection(&connection, "session-drop")
                .unwrap()
                .is_none()
        );
    }

    /** 从测试库读取一条会话 payload，供并发隔离断言复用。 */
    fn load_test_session(connection: &Connection, session_id: &str) -> AgentSession {
        let payload_json: String = connection
            .query_row(
                "SELECT payload_json FROM agent_sessions WHERE id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .unwrap();

        serde_json::from_str(&payload_json).unwrap()
    }

    /** 只 upsert 一个会话时，其它会话的模型、权限和消息必须原样保留。 */
    #[test]
    fn persist_session_records_does_not_delete_or_revert_other_sessions() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("session-upsert.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let mut session_a = test_agent_session("session-a", "2026/01/01 00:00");
        session_a.security_level = "advanced".to_owned();
        session_a.model_provider_id = Some("openai".to_owned());
        session_a.model_id = Some("gpt-4.1".to_owned());
        let mut session_b = test_agent_session("session-b", "2026/01/01 00:01");
        session_b.security_level = "autonomous".to_owned();
        session_b.model_provider_id = Some("anthropic".to_owned());
        session_b.model_id = Some("claude-sonnet-4".to_owned());
        session_b.title = "会话二".to_owned();

        let transaction = connection.transaction().unwrap();
        persist_session_records_in_transaction(
            &transaction,
            &[session_a.clone(), session_b.clone()],
        )
        .unwrap();
        transaction.commit().unwrap();

        session_a.title = "会话一已更新".to_owned();
        let transaction = connection.transaction().unwrap();
        persist_session_records_in_transaction(&transaction, &[session_a.clone()]).unwrap();
        transaction.commit().unwrap();

        let stored_a = load_test_session(&connection, "session-a");
        let stored_b = load_test_session(&connection, "session-b");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_sessions", [], |row| row.get(0))
            .unwrap();

        assert_eq!(count, 2);
        assert_eq!(stored_a.title, "会话一已更新");
        assert_eq!(stored_a.security_level, "advanced");
        assert_eq!(stored_b.title, "会话二");
        assert_eq!(stored_b.security_level, "autonomous");
        assert_eq!(stored_b.model_provider_id.as_deref(), Some("anthropic"));
        assert_eq!(stored_b.model_id.as_deref(), Some("claude-sonnet-4"));
    }

    /** 过期快照整表重写会删掉回合开始后新建的会话；这条路径不能再用于 Agent 落盘。 */
    #[test]
    fn persist_sessions_in_transaction_drops_sessions_missing_from_snapshot() {
        let dir = tempdir().unwrap();
        let database_path = dir.path().join("session-replace.sqlite3");
        let mut connection = Connection::open(&database_path).unwrap();
        ensure_database_schema(&connection, &database_path).unwrap();

        let transaction = connection.transaction().unwrap();
        persist_session_in_transaction(
            &transaction,
            &test_agent_session("session-a", "2026/01/01 00:00"),
        )
        .unwrap();
        persist_session_in_transaction(
            &transaction,
            &test_agent_session("session-b", "2026/01/01 00:01"),
        )
        .unwrap();
        transaction.commit().unwrap();

        let mut stale_snapshot = test_workspace_snapshot();
        stale_snapshot
            .sessions
            .push(test_agent_session("session-a", "2026/01/01 00:00"));
        let transaction = connection.transaction().unwrap();
        persist_sessions_in_transaction(&transaction, &stale_snapshot).unwrap();
        transaction.commit().unwrap();

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_sessions", [], |row| row.get(0))
            .unwrap();
        let has_b: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_sessions WHERE id = ?1",
                ["session-b"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(has_b, 0);
    }
}
