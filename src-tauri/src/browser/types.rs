//! 浏览器 Agent 的 IPC 与运行时 DTO。
//!
//! 这里集中定义前端面板、Tauri 命令和 `run` 工具浏览器分支共用的结构，
//! 避免在命令层散落临时 JSON 形状。所有字段对模型可见时都不包含截图
//! 明细、cookie 或密码明文。

use serde::{Deserialize, Serialize};

/** 面板标签页视图；url/title 由 CDP 实时读取。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabView {
    pub tab_id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub visible: bool,
}

/** 控制租约状态：谁在操作、从何时开始、第几代。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserControlState {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: String,
    pub revision: u64,
    pub elapsed_ms: u128,
}

/** 浏览器整体状态，`browser-state-changed` 事件与状态命令共用。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub available: bool,
    pub enabled: bool,
    pub chromium_installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_error: Option<String>,
    pub tabs: Vec<BrowserTabView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_tab_id: Option<String>,
    pub control: BrowserControlState,
}

/** 面板实时画面帧；data 是 JPEG 的 base64，不进入模型上下文。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFrame {
    pub tab_id: String,
    pub data: String,
    pub width: i64,
    pub height: i64,
}

/** 面板活动提示：Agent 正在做什么。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserActivity {
    pub activity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
}

/** 操作凭证：动作前保存的目标区域快照，只发面板、不进模型上下文。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserReceipt {
    pub receipt_id: String,
    pub tab_id: String,
    pub operation: String,
    /** 目标元素中心点的视口坐标，供面板高亮显示。 */
    pub x: f64,
    pub y: f64,
    pub url: String,
    pub created_at: u64,
}

/**
 * 页面弹窗（alert / confirm / prompt / beforeunload）。
 *
 * 弹窗会阻塞渲染进程，绝大多数 CDP 调用会一起吊住；
 * 模型必须先看到它、再用 page.dialog.accept / dismiss 解开。
 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDialog {
    pub tab_id: String,
    pub dialog_type: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_prompt: Option<String>,
}

/** 审计日志条目：只含 operation、tabId、origin、审批结果、耗时和错误码。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAuditEntry {
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_summary: Option<String>,
    pub approval: String,
    pub duration_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/** 元素快照中的单个元素；密码字段 value 固定为 secret 占位。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserElement {
    pub snapshot_id: String,
    pub reference: String,
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub rect: BrowserRect,
}

/** 元素矩形，视口 CSS 像素坐标。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/** 文本快照的一个分页块。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTextBlock {
    pub id: String,
    pub text: String,
}

/** 页面快照：文本分页 + 元素列表 + 截断标记 + 快照代。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub snapshot_id: String,
    pub page_generation: u64,
    pub captured_at: u64,
    pub blocks: Vec<BrowserTextBlock>,
    pub elements: Vec<BrowserElement>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_text_offset: Option<usize>,
    /** 元素分页续读位置；下一次 page.elements.snapshot 传 elementOffset。 */
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_element_offset: Option<usize>,
}

/** `run` 工具浏览器分支的请求：`{ kind: "browser", operation, ...params }`。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunRequest {
    #[serde(default)]
    pub kind: Option<String>,
    pub operation: String,
    #[serde(flatten)]
    pub params: serde_json::Value,
}

/** `run` 工具浏览器分支返回给模型的结果。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserRunResult {
    pub ok: bool,
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<BrowserTabView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<BrowserSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BrowserError>,
}

/** 结构化错误码；`BROWSER_USER_TAKEOVER` 之后本回合禁止重试。 */
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserError {
    pub code: String,
    pub message: String,
}
