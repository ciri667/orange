use super::agent::AgentSession;
use super::knowledge::{FolderEntry, KnowledgeBase, Note, WorkspaceDocument};
use serde::{Deserialize, Serialize};

/** 前后端传输的工作台快照。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub knowledge_bases: Vec<KnowledgeBase>,
    #[serde(default)]
    pub folders: Vec<FolderEntry>,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub documents: Vec<WorkspaceDocument>,
    pub sessions: Vec<AgentSession>,
    pub active_knowledge_base_id: String,
    pub active_note_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub active_document_id: String,
    pub active_session_id: String,
}

/** 编辑器会话中一个已打开文件的稳定引用；kind 仅允许 note 或 document。 */
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEditorTab {
    pub kind: String,
    pub id: String,
}

/** 持久化的编辑器会话状态；不属于领域快照，避免污染工作区业务数据。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEditorState {
    pub active_knowledge_base_id: String,
    #[serde(default)]
    pub open_tabs: Vec<WorkspaceEditorTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab: Option<WorkspaceEditorTab>,
    pub updated_at: String,
}

/** 应用启动返回值，组合重新扫描后的领域快照与可恢复的编辑器会话。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceBootstrapState {
    pub snapshot: WorkspaceSnapshot,
    pub editor_state: WorkspaceEditorState,
}

/** Agent 单轮返回结果。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResult {
    pub snapshot: WorkspaceSnapshot,
}
