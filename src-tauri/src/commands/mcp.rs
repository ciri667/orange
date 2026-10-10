use tauri::AppHandle;

use crate::domain::{McpApprovalDecision, McpOverview, McpProbeResult, McpServerDraft};
use crate::mcp::{self, manager};

/** 读取 MCP 服务器、连接状态和配置错误。 */
#[tauri::command]
pub async fn load_mcp_overview(app: AppHandle) -> Result<McpOverview, String> {
    let manager = mcp::install(&app).await?;
    Ok(manager.overview())
}

/** 新增或覆盖一台服务器，然后重新连接已启用的条目。 */
#[tauri::command]
pub async fn save_mcp_server(app: AppHandle, draft: McpServerDraft) -> Result<McpOverview, String> {
    let manager = mcp::install(&app).await?;
    manager.save_server(&draft)?;
    manager.reload().await
}

/** 删除一台服务器并重新连接其余已启用条目。 */
#[tauri::command]
pub async fn remove_mcp_server(app: AppHandle, name: String) -> Result<McpOverview, String> {
    let manager = mcp::install(&app).await?;
    manager.remove_named(&name)?;
    manager.reload().await
}

/** 只切换启用状态。关闭后该服务器的工具立刻从模型列表消失。 */
#[tauri::command]
pub async fn set_mcp_server_enabled(
    app: AppHandle,
    name: String,
    enabled: bool,
) -> Result<McpOverview, String> {
    let manager = mcp::install(&app).await?;
    manager.set_enabled_flag(&name, enabled)?;
    manager.reload().await
}

/** 按磁盘上的 mcp.json 重连。 */
#[tauri::command]
pub async fn reload_mcp_servers(app: AppHandle) -> Result<McpOverview, String> {
    let manager = mcp::install(&app).await?;
    manager.reload().await
}

/** 试连一次，不改变启用状态，连完就关掉。 */
#[tauri::command]
pub async fn test_mcp_server(app: AppHandle, name: String) -> Result<McpProbeResult, String> {
    let manager = mcp::install(&app).await?;
    manager.probe(&name).await
}

/** 把进阶级别弹窗的选择送回正在等待的回合。 */
#[tauri::command]
pub async fn decide_mcp_tool_call(app: AppHandle, decision: McpApprovalDecision) -> Result<(), String> {
    let Some(manager) = manager(&app) else {
        return Err("MCP 还没有启动。".to_owned());
    };
    if !manager.decide(&decision.approval_id, decision.approved) {
        return Err("这次确认已经结束。".to_owned());
    }
    Ok(())
}
