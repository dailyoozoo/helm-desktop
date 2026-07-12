//! 真实自动更新（P2-1）：基于 tauri-plugin-updater。
//! 发布源 URL 保存在应用设置（`general.updateFeedUrl`），运行时动态注入 endpoints，
//! 签名公钥固定在 tauri.conf.json（`plugins.updater.pubkey`）。

use crate::sessions::SessionHistoryStore;
use crate::settings::load_app_settings_from_store;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;

/// 前端「检查更新」的结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
}

fn feed_url_from_settings(history_store: &SessionHistoryStore) -> Result<String, String> {
    let settings = load_app_settings_from_store(history_store)?;
    let feed = settings.general.update_feed_url.trim().to_string();
    if feed.is_empty() {
        return Err("尚未配置更新发布源；请先在设置 → 通用里填写 latest.json 地址".to_string());
    }
    Ok(feed)
}

fn build_updater(app: &AppHandle, feed_url: &str) -> Result<tauri_plugin_updater::Updater, String> {
    let url = feed_url
        .parse()
        .map_err(|e| format!("发布源 URL 无效（{feed_url}）：{e}"))?;
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| format!("配置发布源失败：{e}"))?
        .build()
        .map_err(|e| format!("初始化更新器失败：{e}"))
}

/// 检查更新：真实请求发布源并做签名/版本比较（由插件完成）
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<UpdateCheckResult, String> {
    let feed = feed_url_from_settings(&history_store)?;
    let updater = build_updater(&app, &feed)?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败：{e}"))?;

    let current_version = env!("CARGO_PKG_VERSION").to_string();
    Ok(match update {
        Some(update) => UpdateCheckResult {
            current_version,
            available: true,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
        },
        None => UpdateCheckResult {
            current_version,
            available: false,
            version: None,
            notes: None,
        },
    })
}

/// 下载并安装更新：进度经 `update-progress` 事件推给前端；安装完成后重启应用
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<(), String> {
    let feed = feed_url_from_settings(&history_store)?;
    let updater = build_updater(&app, &feed)?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败：{e}"))?
        .ok_or_else(|| "当前已是最新版本".to_string())?;

    let progress_app = app.clone();
    let mut downloaded: u64 = 0;
    let finished_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress_app.emit(
                    "update-progress",
                    serde_json::json!({ "downloaded": downloaded, "total": total }),
                );
            },
            move || {
                let _ =
                    finished_app.emit("update-progress", serde_json::json!({ "finished": true }));
            },
        )
        .await
        .map_err(|e| format!("下载或安装更新失败：{e}"))?;

    // Windows 上安装器会接管并退出应用；其余平台显式重启加载新版本
    app.restart();
}
