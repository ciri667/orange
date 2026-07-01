mod agent;
mod agent_tools;
mod commands;
mod domain;
mod export;
mod logging;
mod model_provider;
mod runtime;
mod skills;
mod storage;
mod text_edit;

/** 桌面端应用入口，注册本地文件、索引、Agent loop 和写入确认命令。 */
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(14))
                .max_file_size(1_048_576)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let handle = app.handle().clone();

            logging::write_app_event_best_effort(
                &handle,
                logging::AppEventBuilder::new(
                    logging::AppLogLevel::Info,
                    logging::AppLogCategory::App,
                    "app_started",
                    "completed",
                    "Cici Note 桌面端已启动。",
                ),
            );

            match logging::cleanup_old_file_logs(&handle) {
                Ok(removed_count) if removed_count > 0 => logging::write_app_event_best_effort(
                    &handle,
                    logging::AppEventBuilder::new(
                        logging::AppLogLevel::Info,
                        logging::AppLogCategory::App,
                        "file_log_cleanup",
                        "completed",
                        "已清理过期文件日志。",
                    )
                    .metadata(serde_json::json!({ "removedCount": removed_count })),
                ),
                Ok(_) => {}
                Err(error) => logging::write_app_event_best_effort(
                    &handle,
                    logging::AppEventBuilder::new(
                        logging::AppLogLevel::Warn,
                        logging::AppLogCategory::App,
                        "file_log_cleanup",
                        "failed",
                        error,
                    ),
                ),
            }

            Ok(())
        })
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
            commands::install_agent_skill,
            commands::save_model_api_key,
            commands::load_model_api_key_statuses,
            commands::load_llm_provider_templates,
            commands::load_request_audit_logs,
            commands::load_app_event_logs,
            commands::clear_app_event_logs,
            commands::open_app_log_folder,
            commands::scan_knowledge_base,
            commands::rescan_knowledge_base,
            commands::create_note,
            commands::create_document,
            commands::create_folder,
            commands::rename_note,
            commands::rename_document,
            commands::delete_note,
            commands::delete_document,
            commands::save_note_content,
            commands::save_note_image_attachments,
            commands::save_document_content,
            commands::load_document_preview,
            export::export_current_file,
            commands::remove_knowledge_base,
            commands::run_agent_turn,
            commands::apply_proposed_change,
            commands::reject_proposed_change
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Cici Note desktop app");
}
