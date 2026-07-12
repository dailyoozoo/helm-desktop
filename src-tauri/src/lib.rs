//! Helm 桌面后端入口（Tauri 2）。组装应用：注册会话存储与命令、加载配置启动。

pub mod adapter;
pub mod commands;
pub mod extensions;
pub mod installer;
pub mod parse;
pub mod protocol;
pub mod providers;
pub mod sessions;
pub mod settings;
pub mod snapshots;
pub mod titler;
pub mod tray;
pub mod updater;
pub mod worktree;

use commands::{
    approval_response, check_budget, close_session, create_session, delete_hook, delete_mcp_server,
    delete_provider_config, delete_session, delete_slash_command, delete_subagent,
    get_active_session, get_always_allow_tools, get_budget, get_daily_usage, get_equivalent_env,
    get_provider_config, get_session_history, get_top_sessions, get_usage_by_model,
    get_usage_by_provider, get_usage_stats, interrupt, list_hooks, list_mcp_servers, list_sessions,
    list_skills, list_slash_commands, list_subagents, market_install_skill, market_search_skills,
    read_engine_config_file, remove_always_allow_tool, rename_session, restore_checkpoint,
    resume_session, reveal_provider_secret, save_binding_config, save_engine_config, save_hook,
    save_mcp_server, save_model_config, save_pasted_image, save_provider_config,
    save_slash_command, save_subagent, search_workspace_files, send_message, set_budget,
    set_provider_defaults, set_session_mcp_disabled, set_session_pinned,
    sync_provider_models_config, test_engine_config, test_mcp_connection, test_provider_config,
    toggle_skill, undo_revert, write_engine_config_file, SessionStore,
};
use installer::install_cli_engine;
use providers::{KeyringSecretStore, ProviderStore};
use sessions::SessionHistoryStore;
use settings::{
    detect_cli_engine, detect_cli_login, export_app_settings, get_readiness_report,
    get_update_status, import_app_settings, load_app_settings, load_app_settings_from_store,
    save_app_settings, select_directory,
};
use tauri::Manager;
use updater::{check_for_update, install_update};
use worktree::{create_session_worktree, remove_session_worktree};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        // 窗口尺寸/位置记忆（变更-12）：官方 window-state 插件，关闭时保存、启动时还原
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|e| format!("获取配置目录失败：{e}"))?;
            app.manage(ProviderStore::new(
                config_dir.join("providers.json"),
                KeyringSecretStore,
            ));
            app.manage(SessionHistoryStore::new(config_dir.join("sessions.sqlite")));
            // 启动归位（变更-12）：应用刚启动不可能有运行中的轮次，
            // 把强杀/崩溃留下的 active 尸体会话归位为 idle
            if let Some(history) = app.try_state::<SessionHistoryStore>() {
                if let Err(err) = history.normalize_stale_active_sessions() {
                    eprintln!("[helm] 启动归位会话状态失败：{err}");
                }
            }
            // 用量托盘（P3-2）：常驻托盘失败不阻断主窗口启动，只留诊断日志
            if let Err(err) = tray::setup(app) {
                eprintln!("[helm] 初始化系统托盘失败：{err}");
            }
            Ok(())
        })
        .manage(SessionStore::default())
        .invoke_handler(tauri::generate_handler![
            create_session,
            close_session,
            delete_session,
            rename_session,
            set_session_pinned,
            list_sessions,
            get_active_session,
            get_session_history,
            resume_session,
            send_message,
            approval_response,
            set_session_mcp_disabled,
            search_workspace_files,
            save_pasted_image,
            get_always_allow_tools,
            remove_always_allow_tool,
            interrupt,
            restore_checkpoint,
            undo_revert,
            get_provider_config,
            reveal_provider_secret,
            save_provider_config,
            delete_provider_config,
            save_engine_config,
            save_model_config,
            save_binding_config,
            get_equivalent_env,
            read_engine_config_file,
            write_engine_config_file,
            set_provider_defaults,
            test_provider_config,
            sync_provider_models_config,
            test_engine_config,
            get_usage_stats,
            get_usage_by_model,
            get_usage_by_provider,
            get_daily_usage,
            get_top_sessions,
            get_budget,
            set_budget,
            check_budget,
            list_skills,
            toggle_skill,
            list_mcp_servers,
            test_mcp_connection,
            save_mcp_server,
            delete_mcp_server,
            list_subagents,
            save_subagent,
            delete_subagent,
            list_slash_commands,
            save_slash_command,
            delete_slash_command,
            list_hooks,
            save_hook,
            delete_hook,
            market_search_skills,
            market_install_skill,
            load_app_settings,
            save_app_settings,
            export_app_settings,
            import_app_settings,
            get_update_status,
            check_for_update,
            install_update,
            detect_cli_engine,
            detect_cli_login,
            install_cli_engine,
            get_readiness_report,
            create_session_worktree,
            remove_session_worktree,
            select_directory
        ])
        .on_window_event(|window, event| {
            // 关闭行为（变更-12）：closeToTray 开启 → 隐藏到托盘（后台会话继续跑）；
            // 否则有会话在跑时先确认，不再静默杀掉后台轮次
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let close_to_tray = window
                    .app_handle()
                    .try_state::<SessionHistoryStore>()
                    .and_then(|store| load_app_settings_from_store(&store).ok())
                    .map(|settings| settings.general.close_to_tray)
                    .unwrap_or(false);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                    return;
                }
                if adapter::has_running_processes() {
                    api.prevent_close();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        use tauri_plugin_dialog::{
                            DialogExt, MessageDialogButtons, MessageDialogKind,
                        };
                        let confirmed = window
                            .dialog()
                            .message(
                                "还有会话正在运行，退出会终止这些后台任务。\n确定要退出 Helm 吗？",
                            )
                            .title("退出 Helm")
                            .kind(MessageDialogKind::Warning)
                            .buttons(MessageDialogButtons::OkCancelCustom(
                                "退出".to_string(),
                                "取消".to_string(),
                            ))
                            .blocking_show();
                        if confirmed {
                            let _ = window.destroy();
                        }
                    });
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Helm 启动失败")
        .run(|_app, event| {
            // 退出前同步杀掉所有仍在运行的 CLI 进程树，避免 Windows 上留下孤儿 node 进程。
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                adapter::kill_all_running_processes();
            }
        });
}
