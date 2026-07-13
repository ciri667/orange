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

use tauri::{Manager, WindowEvent};

/** 托盘菜单 ID：显式退出才会停止本机 IM 网关，关闭窗口只隐藏到后台。 */
const TRAY_MENU_SHOW: &str = "orange-show-window";
const TRAY_MENU_QUIT: &str = "orange-quit-app";

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
            let show_item = tauri::menu::MenuItem::with_id(
                app,
                TRAY_MENU_SHOW,
                "显示橘记",
                true,
                None::<&str>,
            )?;
            let quit_item = tauri::menu::MenuItem::with_id(
                app,
                TRAY_MENU_QUIT,
                "退出橘记",
                true,
                None::<&str>,
            )?;
            let tray_menu = tauri::menu::Menu::with_items(app, &[&show_item, &quit_item])?;
            let mut tray_builder = tauri::tray::TrayIconBuilder::with_id("orange-im-gateway")
                .menu(&tray_menu)
                .tooltip("橘记正在后台运行，飞书远程服务可用")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    TRAY_MENU_SHOW => {
                        // 从托盘恢复窗口时同时请求焦点，避免窗口被其他应用遮挡后像是未响应。
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    TRAY_MENU_QUIT => app.exit(0),
                    _ => {}
                });

            // 复用应用默认图标；开发环境没有默认图标时仍保留菜单和后台能力。
            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            tray_builder.build(app)?;

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
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 关闭主窗口不退出进程，使已启动的 IM sidecar 能持续接收远程确认操作。
                api.prevent_close();
                if let Err(error) = window.hide() {
                    logging::write_app_event_best_effort(
                        &window.app_handle(),
                        logging::AppEventBuilder::new(
                            logging::AppLogLevel::Warn,
                            logging::AppLogCategory::App,
                            "app_window_hide",
                            "failed",
                            error.to_string(),
                        ),
                    );
                }
            }
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
            commands::load_knowledge_base_memories,
            commands::save_knowledge_base_memory,
            commands::delete_knowledge_base_memory,
            commands::save_model_api_key,
            commands::load_model_api_key_statuses,
            commands::load_llm_provider_templates,
            commands::refresh_llm_provider_models,
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
            commands::load_document_history,
            commands::load_document_history_entry,
            commands::restore_document_history_entry,
            commands::clear_document_history,
            commands::load_document_preview,
            export::export_current_file,
            commands::run_agent_turn,
            commands::compact_agent_context,
            commands::apply_proposed_change,
            commands::reject_proposed_change
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Orange desktop app");
}
