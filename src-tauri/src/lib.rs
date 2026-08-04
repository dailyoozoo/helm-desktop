//! Helm 桌面后端入口（Tauri 2）。组装应用：注册会话存储与命令、加载配置启动。

pub mod adapter;
pub mod budget;
pub mod capability_registry;
pub mod claude_capabilities;
pub mod claude_permission_hook;
pub mod codex_app_server;
pub mod codex_capabilities;
pub mod commands;
pub mod extensions;
pub mod git;
pub mod handoff;
pub mod installer;
pub mod operations;
pub mod parse;
pub mod permission_kernel;
pub mod permission_service;
pub mod permissions;
pub mod pricing;
pub mod protocol;
pub mod providers;
pub mod reasoning;
mod redaction;
pub mod runtime_registry;
pub mod sandbox_ceiling;
pub mod session_actor;
pub mod session_context;
pub mod sessions;
pub mod settings;
pub mod snapshots;
pub mod subscription_profiles;
pub mod titler;
pub mod tray;
pub mod turn_start;
pub mod turn_supervisor;
pub mod updater;
pub mod util;
pub mod workspace_execution;

#[cfg(test)]
mod tests {
    use crate::permission_service::PermissionService;
    use crate::sessions::SessionHistoryStore;

    #[tokio::test]
    async fn permission_service_starts_and_reports_running() {
        let database = std::env::temp_dir().join(format!(
            "helm-permission-backends-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = SessionHistoryStore::new(database.clone());

        let service = PermissionService::start(store).await.unwrap();

        assert_ne!(service.addr().port(), 0);

        service.shutdown().await;
        let _ = std::fs::remove_file(database);
    }
}

use capability_registry::EngineCapabilityRegistry;
use commands::{
    add_session_context, approval_response, cancel_background_operation, clear_permission_audit,
    close_session, create_folder, create_permission_deny_rule, create_session, delete_folder,
    delete_hook, delete_mcp_server, delete_provider_config, delete_session, delete_slash_command,
    delete_subagent, export_permission_audit, get_active_session, get_background_operation,
    get_budget, get_daily_usage, get_equivalent_env, get_permission_audit_summary,
    get_permission_rules, get_provider_config, get_reasoning_effort_capability,
    get_session_history, get_top_sessions, get_turn_snapshot, get_usage_by_model,
    get_usage_by_provider, get_usage_stats, interrupt, list_folders, list_hooks, list_mcp_servers,
    list_session_contexts, list_sessions, list_skills, list_slash_commands, list_subagents,
    market_install_skill, market_search_skills, read_engine_config_file, remove_permission_rule,
    remove_session_context, rename_folder, rename_session, restore_checkpoint, resume_session,
    retry_background_operation, reveal_provider_secret, save_binding_config, save_engine_config,
    save_hook, save_mcp_server, save_model_config, save_pasted_image, save_provider_config,
    save_provider_model_selection, save_slash_command, save_subagent, search_workspace_files,
    send_message, set_budget, set_folder_collapsed, set_session_folder, set_session_mcp_disabled,
    set_session_permission_profile, set_session_pinned, set_session_turn_preference,
    start_session_fork, sync_provider_models_config, test_engine_config, test_mcp_connection,
    test_provider_config, toggle_skill, undo_revert, write_engine_config_file, SessionStore,
};
use installer::install_cli_engine;
use permission_service::PermissionService;
use pricing::PricingCatalogStore;
use pricing::{
    delete_model_price_override, get_pricing_catalog_status, get_provider_pricing_preference,
    import_pricing_catalog, list_model_price_overrides, refresh_pricing_catalog,
    save_model_price_override, save_provider_pricing_preference,
};
use providers::{KeyringSecretStore, ProviderStore};
use runtime_registry::RuntimeRegistry;
use sessions::SessionHistoryStore;
use settings::{
    detect_cli_engine, detect_cli_login, export_app_settings, get_readiness_report,
    get_update_status, import_app_settings, load_app_settings, load_app_settings_from_store,
    login_cli_account, logout_cli_account, save_app_settings, select_directory,
};
use subscription_profiles::SubscriptionProfileStore;
use tauri::Manager;
use turn_supervisor::TurnSupervisor;
use updater::{check_for_update, install_update};

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
            app.manage(SubscriptionProfileStore::new(config_dir.clone()));
            let pricing_store = PricingCatalogStore::new(config_dir.clone());
            app.manage(pricing_store.clone());
            let history_store = SessionHistoryStore::new(config_dir.join("sessions.sqlite"));
            app.manage(adapter::CodexRuntimeProfileStore::new(
                config_dir.clone(),
                history_store.clone(),
            ));
            app.manage(EngineCapabilityRegistry::new(history_store.clone()));
            let permission_service =
                tauri::async_runtime::block_on(PermissionService::start(history_store.clone()))
                    .map_err(|error| format!("初始化权限服务失败：{error}"))?;
            let recovery = history_store
                .reconcile_stream_recovery()
                .map_err(|error| format!("Stream Supervisor 启动恢复失败：{error}"))?;
            if recovery != Default::default() {
                eprintln!("[helm] Stream Supervisor 启动恢复：{recovery:?}");
            }
            let operation_recovery = history_store
                .reconcile_background_operations()
                .map_err(|error| format!("BackgroundOperation 启动恢复失败：{error}"))?;
            if operation_recovery > 0 {
                eprintln!(
                    "[helm] BackgroundOperation 启动恢复：{} 个不确定 Attempt 已收口为 delivery_unknown",
                    operation_recovery
                );
            }
            let turn_supervisor =
                TurnSupervisor::with_app(history_store.clone(), app.handle().clone());
            let runtime_registry =
                RuntimeRegistry::with_supervisor(history_store.clone(), turn_supervisor.clone())
                    .map_err(|error| format!("初始化 RuntimeRegistry 失败：{error}"))?;
            if !runtime_registry.recovery_inputs().is_empty() {
                eprintln!(
                    "[helm] 检测到 {} 个未收口 TurnAttempt，已加载为 27F 恢复输入",
                    runtime_registry.recovery_inputs().len()
                );
            }
            app.manage(turn_supervisor);
            app.manage(runtime_registry);
            app.manage(history_store);
            app.manage(permission_service);
            // 启动归位（变更-12）：应用刚启动不可能有运行中的轮次，
            // 把强杀/崩溃留下的 active 尸体会话归位为 idle
            if let Some(history) = app.try_state::<SessionHistoryStore>() {
                if let Err(err) = history.normalize_stale_active_sessions() {
                    eprintln!("[helm] 启动归位会话状态失败：{err}");
                }
            }
            if let Some(history) = app.try_state::<SessionHistoryStore>() {
                pricing::spawn_background_refresh(
                    app.handle().clone(),
                    pricing_store,
                    history.inner().clone(),
                );
            }
            // 用量托盘（P3-2）：常驻托盘失败不阻断主窗口启动，只留诊断日志
            if let Err(err) = tray::setup(app) {
                eprintln!("[helm] 初始化系统托盘失败：{err}");
            }
            Ok(())
        })
        .manage(SessionStore::default())
        .manage(workspace_execution::WorkspaceExecutionCoordinator::default())
        .invoke_handler(tauri::generate_handler![
            create_session,
            list_folders,
            create_folder,
            rename_folder,
            delete_folder,
            set_session_folder,
            set_folder_collapsed,
            close_session,
            delete_session,
            rename_session,
            set_session_pinned,
            list_sessions,
            get_active_session,
            get_session_history,
            list_session_contexts,
            add_session_context,
            remove_session_context,
            resume_session,
            send_message,
            set_session_permission_profile,
            set_session_turn_preference,
            approval_response,
            set_session_mcp_disabled,
            search_workspace_files,
            save_pasted_image,
            get_permission_rules,
            create_permission_deny_rule,
            remove_permission_rule,
            get_permission_audit_summary,
            clear_permission_audit,
            export_permission_audit,
            interrupt,
            get_background_operation,
            start_session_fork,
            cancel_background_operation,
            retry_background_operation,
            get_turn_snapshot,
            restore_checkpoint,
            undo_revert,
            get_provider_config,
            reveal_provider_secret,
            save_provider_config,
            delete_provider_config,
            save_engine_config,
            save_model_config,
            save_provider_model_selection,
            save_binding_config,
            get_equivalent_env,
            read_engine_config_file,
            write_engine_config_file,
            test_provider_config,
            sync_provider_models_config,
            test_engine_config,
            get_reasoning_effort_capability,
            get_usage_stats,
            get_usage_by_model,
            get_usage_by_provider,
            get_daily_usage,
            get_top_sessions,
            get_budget,
            set_budget,
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
            get_pricing_catalog_status,
            refresh_pricing_catalog,
            import_pricing_catalog,
            list_model_price_overrides,
            save_model_price_override,
            delete_model_price_override,
            get_provider_pricing_preference,
            save_provider_pricing_preference,
            detect_cli_engine,
            detect_cli_login,
            login_cli_account,
            logout_cli_account,
            install_cli_engine,
            get_readiness_report,
            select_directory,
            git::get_git_branch,
            git::get_git_status,
            git::get_git_staged
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
        .run(|app, event| {
            // 退出前同步杀掉所有仍在运行的 CLI 进程树，避免 Windows 上留下孤儿 node 进程。
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                tauri::async_runtime::block_on(async {
                    if let Some(registry) = app.try_state::<RuntimeRegistry>() {
                        registry.shutdown_all().await;
                    }
                    if let Some(service) = app.try_state::<PermissionService>() {
                        service.shutdown().await;
                    }
                });
                // Registry 是正常回收路径；PID 表只保留为同步兜底，处理 Runtime 已失联的进程树。
                adapter::kill_all_running_processes();
            }
        });
}
