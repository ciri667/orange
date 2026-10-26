pub(crate) mod naming;

mod client;
mod config;
mod manager;
mod policy;
mod secrets;

pub use client::kill_tracked_processes;
pub use manager::{execute_model_tool, install, manager, prepare_turn_schemas};

/** 真正退出时拆掉 stdio 子进程。关窗口只是藏到托盘，不走这里。 */
pub fn shutdown_processes() {
    kill_tracked_processes();
}
