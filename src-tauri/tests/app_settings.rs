use helm_lib::sessions::SessionHistoryStore;
use helm_lib::settings::{
    export_app_settings_to_path, import_app_settings_from_path, load_app_settings_from_store,
    save_app_settings_to_store, update_status_from_settings, AppSettings,
};
use std::fs;

fn temp_history_path(name: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "helm-app-settings-{}-{name}.sqlite",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    path
}

#[test]
fn app_settings_use_defaults_when_missing() {
    let path = temp_history_path("defaults");
    let store = SessionHistoryStore::new(path);

    let settings = load_app_settings_from_store(&store).unwrap();

    assert_eq!(settings.engines.default_engine, "claude-code");
    assert!(settings.general.pricing_auto_update);
    assert_eq!(settings.general.pricing_unknown_policy, "warn");
    assert_eq!(settings.general.pricing_max_age_days, 30);
}

#[test]
fn app_settings_round_trip_through_sqlite_setting_table() {
    let path = temp_history_path("round-trip");
    let store = SessionHistoryStore::new(path.clone());
    let mut settings = AppSettings::default();
    settings.general.workspace_name = "真实设置工作区".to_string();
    settings.general.default_directory = "D:\\work\\helm".to_string();
    settings.engines.default_engine = "codex".to_string();
    settings.appearance.theme = "dark".to_string();
    settings.general.pricing_feed_urls = vec![
        "https://oss.example.cn/helm/pricing-catalog.json".to_string(),
        "https://cos.example.cn/helm/pricing-catalog.json".to_string(),
    ];
    settings.general.pricing_unknown_policy = "block".to_string();

    save_app_settings_to_store(&store, settings.clone()).unwrap();

    let loaded = load_app_settings_from_store(&store).unwrap();
    assert_eq!(loaded.general.workspace_name, "真实设置工作区");
    assert_eq!(loaded.general.default_directory, "D:\\work\\helm");
    assert_eq!(loaded.engines.default_engine, "codex");
    assert_eq!(loaded.appearance.theme, "dark");
    assert_eq!(loaded.general.pricing_feed_urls.len(), 2);
    assert_eq!(loaded.general.pricing_unknown_policy, "block");

    let conn = rusqlite::Connection::open(path).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT value_json FROM setting WHERE key = 'app_settings'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["general"]["workspaceName"], "真实设置工作区");
    assert_eq!(value["engines"]["defaultEngine"], "codex");
}

#[test]
fn update_status_reports_channel_without_claiming_network_checks() {
    let mut settings = AppSettings::default();
    settings.general.auto_update_channel = "beta".to_string();

    let status = update_status_from_settings(&settings);

    assert_eq!(status.channel, "beta");
    assert!(!status.can_check);
    assert_eq!(status.current_version, env!("CARGO_PKG_VERSION"));
    assert!(status.message.contains("未配置自动更新发布源"));
}

#[test]
fn update_status_reports_configured_release_source() {
    let mut settings = AppSettings::default();
    settings.general.auto_update_channel = "stable".to_string();
    settings.general.update_feed_url = "https://updates.example.com/helm/latest.json".to_string();

    let status = update_status_from_settings(&settings);

    assert_eq!(status.channel, "stable");
    assert!(status.can_check);
    assert!(status
        .message
        .contains("https://updates.example.com/helm/latest.json"));
}

#[test]
fn app_settings_import_export_round_trips_json_file() {
    let db_path = temp_history_path("import-export");
    let store = SessionHistoryStore::new(db_path);
    let mut settings = AppSettings::default();
    settings.general.workspace_name = "导出工作区".to_string();
    settings.shortcuts.new_session = "Ctrl+Shift+N".to_string();
    save_app_settings_to_store(&store, settings).unwrap();

    let export_path =
        std::env::temp_dir().join(format!("helm-settings-export-{}.json", std::process::id()));
    let _ = fs::remove_file(&export_path);
    export_app_settings_to_path(&store, &export_path).unwrap();

    let mut replacement = AppSettings::default();
    replacement.general.workspace_name = "导入工作区".to_string();
    replacement.general.update_feed_url = "https://updates.example.com/appcast.json".to_string();
    fs::write(
        &export_path,
        serde_json::to_string_pretty(&replacement).unwrap(),
    )
    .unwrap();

    let imported = import_app_settings_from_path(&store, &export_path).unwrap();

    assert_eq!(imported.general.workspace_name, "导入工作区");
    assert_eq!(
        load_app_settings_from_store(&store)
            .unwrap()
            .general
            .update_feed_url,
        "https://updates.example.com/appcast.json"
    );

    let _ = fs::remove_file(export_path);
}

#[test]
fn always_allow_tools_persist_across_store_instances() {
    // P2-4：「始终允许」跨会话持久化——写入后重开 store（模拟重启应用）仍能读到
    let path = temp_history_path("always-allow");
    {
        let store = SessionHistoryStore::new(path.clone());
        assert!(store.get_always_allow_tools().unwrap().is_empty());
        store.add_always_allow_tool("Bash").unwrap();
        store.add_always_allow_tool("Write").unwrap();
        // 重复添加不产生重复项
        store.add_always_allow_tool("Bash").unwrap();
    }

    let reopened = SessionHistoryStore::new(path);
    assert_eq!(
        reopened.get_always_allow_tools().unwrap(),
        vec!["Bash".to_string(), "Write".to_string()]
    );

    let after_remove = reopened.remove_always_allow_tool("Bash").unwrap();
    assert_eq!(after_remove, vec!["Write".to_string()]);
    assert_eq!(
        reopened.get_always_allow_tools().unwrap(),
        vec!["Write".to_string()]
    );
}
