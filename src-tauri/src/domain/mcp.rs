use serde::{Deserialize, Serialize};

/** 设置页看到的一台 MCP 服务器。密钥不在这里。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    pub name: String,
    pub enabled: bool,
    pub transport: String,
    pub state: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub has_authorization: bool,
    pub error: Option<String>,
    pub stderr_tail: String,
    pub exposed_tool_count: usize,
    pub tools: Vec<McpToolView>,
}

/** 服务器上的一个工具，以及它现在会不会进入模型工具列表。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolView {
    pub raw_name: String,
    pub public_name: String,
    pub description: String,
    pub read_only: bool,
    pub exposed: bool,
}

/** 配置文件里被跳过的条目。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfigIssue {
    pub server_name: String,
    pub message: String,
}

/** 设置页一次读取的 MCP 概览。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOverview {
    pub config_path: String,
    pub servers: Vec<McpServerView>,
    pub issues: Vec<McpConfigIssue>,
}

/** 设置页保存一台服务器。令牌只在这次请求里出现，不写回前端状态。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerDraft {
    pub name: String,
    pub enabled: bool,
    pub transport: String,
    pub command: String,
    pub args_text: String,
    pub url: String,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub env_text: String,
    pub headers_text: String,
    pub disabled_tools: Vec<String>,
    /** Some 表示用户改了令牌；空字符串表示清除。None 表示保持原样。 */
    #[serde(default)]
    pub authorization_token: Option<String>,
}

/** 测试连接的结果，不改变启用状态。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProbeResult {
    pub server_name: String,
    pub ok: bool,
    pub message: String,
    pub tool_names: Vec<String>,
}

/** 回合中等待用户确认的一次 MCP 调用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpApprovalRequest {
    pub approval_id: String,
    pub session_id: String,
    pub server_name: String,
    pub tool_name: String,
    pub arguments_preview: String,
}

/** 前端对确认弹窗的决定。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpApprovalDecision {
    pub approval_id: String,
    pub approved: bool,
}
