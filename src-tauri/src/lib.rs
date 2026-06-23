mod agent;
mod agent_tools;
mod commands;
mod domain;
mod runtime;
mod skills;
mod storage;
mod text_edit;

/** 桌面端应用入口，注册本地文件、索引、Agent loop 和写入确认命令。 */
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::load_workspace_state,
            commands::select_knowledge_base,
            commands::load_sessions,
            commands::save_session,
            commands::delete_session,
            commands::update_session_scope,
            commands::restore_session_context,
            commands::load_user_settings,
            commands::save_user_settings,
            commands::load_agent_skills,
            commands::open_user_skills_folder,
            commands::save_agent_skill,
            commands::toggle_agent_skill,
            commands::delete_agent_skill,
            commands::save_model_api_key,
            commands::load_model_api_key_status,
            commands::load_request_audit_logs,
            commands::scan_knowledge_base,
            commands::rescan_knowledge_base,
            commands::create_note,
            commands::create_folder,
            commands::rename_note,
            commands::delete_note,
            commands::save_note_content,
            commands::remove_knowledge_base,
            commands::run_agent_turn,
            commands::apply_proposed_change,
            commands::reject_proposed_change
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Cici Note desktop app");
}
