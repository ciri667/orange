//! 用户终端 IPC 适配；阻塞 PTY IO 不占用 Tauri 异步执行线程。
use super::common::*;
use crate::terminal::{self, TerminalDescriptor, TerminalEvent};

#[tauri::command]
pub async fn terminal_create(app: AppHandle, knowledge_base_id: String, cols: u16, rows: u16, on_event: tauri::ipc::Channel<TerminalEvent>) -> Result<TerminalDescriptor, String> {
    run_blocking("创建终端", move || terminal::create(&app, &knowledge_base_id, cols, rows, on_event)).await
}
#[tauri::command]
pub async fn terminal_input(terminal_id: String, data: String) -> Result<(), String> {
    run_blocking("终端输入", move || terminal::input(&terminal_id, &data)).await
}
#[tauri::command]
pub async fn terminal_resize(terminal_id: String, cols: u16, rows: u16) -> Result<(), String> {
    run_blocking("调整终端", move || terminal::resize(&terminal_id, cols, rows)).await
}
#[tauri::command]
pub async fn terminal_close(terminal_id: String) -> Result<(), String> {
    run_blocking("关闭终端", move || terminal::close(&terminal_id)).await
}
