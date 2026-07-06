mod agent;
mod agent_tools;
mod commands;
mod domain;
mod export;
mod im;
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
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();

            logging::write_app_event_best_effort(
                &handle,
                logging::AppEventBuilder::new(
                    logging::AppLogLevel::Info,
                    logging::AppLogCategory::App,
                    "app_started",
                    "completed",
                    "橘记 桌面端已启动。",
                ),
            );

            // 品牌升级（cici-note → orange）的一次性本地数据迁移：幂等、best-effort。
            skills::migrate_legacy_cici_data(&handle);

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

            let im_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                match storage::load_im_settings(&im_handle) {
                    Ok(settings) => {
                        for provider in settings
                            .providers
                            .iter()
                            .filter(|provider| provider.enabled)
                        {
                            let provider_id = provider.provider_id.clone();
                            // 自动启动只按 provider 路由；具体凭证校验和 sidecar 启动由各 provider 负责。
                            if let Err(error) =
                                im::start_gateway(im_handle.clone(), &provider_id).await
                            {
                                logging::write_app_event_best_effort(
                                    &im_handle,
                                    logging::AppEventBuilder::new(
                                        logging::AppLogLevel::Warn,
                                        logging::AppLogCategory::Im,
                                        "im_gateway_autostart",
                                        "failed",
                                        error,
                                    )
                                    .metadata(serde_json::json!({ "providerId": provider_id })),
                                );
                            }
                        }
                    }
                    Err(error) => logging::write_app_event_best_effort(
                        &im_handle,
                        logging::AppEventBuilder::new(
                            logging::AppLogLevel::Warn,
                            logging::AppLogCategory::Im,
                            "im_gateway_autostart",
                            "failed",
                            error,
                        ),
                    ),
                }
            });

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
            commands::load_im_settings,
            commands::save_im_settings,
            commands::save_im_provider_secret,
            commands::load_im_provider_credential_status,
            commands::start_im_gateway,
            commands::stop_im_gateway,
            commands::load_im_gateway_status,
            commands::save_feishu_app_secret,
            commands::load_feishu_credential_status,
            commands::start_feishu_gateway,
            commands::stop_feishu_gateway,
            commands::load_feishu_gateway_status,
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
            commands::remove_knowledge_base,
            commands::rename_note,
            commands::delete_note,
            commands::save_note_content,
            commands::create_note,
            commands::save_note_image_attachments,
            commands::rename_document,
            commands::delete_document,
            commands::save_document_content,
            commands::create_document,
            commands::create_folder,
            commands::load_document_preview,
            export::export_current_file,
            commands::run_agent_turn,
            commands::apply_proposed_change,
            commands::reject_proposed_change
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Orange desktop app");
}
