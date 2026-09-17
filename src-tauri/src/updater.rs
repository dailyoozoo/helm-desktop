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
    /// "feed" = 已配置签名发布源，可应用内下载安装；"github" = 仅比对到 GitHub 新版本，需前往发布页下载。
    pub source: &'static str,
    /// GitHub 回退时的 releases 页面地址。
    pub release_url: Option<String>,
}

fn feed_url_from_settings(history_store: &SessionHistoryStore) -> Result<String, String> {
    let settings = load_app_settings_from_store(history_store)?;
    let feed = settings.general.update_feed_url.trim().to_string();
    if feed.is_empty() {
        return Err("尚未配置更新发布源；请先在设置 → 通用里填写 latest.json 地址".to_string());
    }
    Ok(feed)
}

/// GitHub releases/latest 下载量最小、无需凭据的版本检查回退：
/// 与大多数开源产品一致——只比较版本号并引导到发布页，不做应用内静默安装
/// （静默安装必须走带 minisign 验签的 latest.json，防止供应链投毒）。
const GITHUB_LATEST_API: &str =
    "https://api.github.com/repos/dailyoozoo/helm-desktop/releases/latest";
const GITHUB_RELEASES: &str = "https://api.github.com/repos/dailyoozoo/helm-desktop/releases";

#[derive(Debug, Clone, serde::Deserialize)]
struct GitHubLatestRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
}

async fn fetch_github_latest() -> Result<GitHubLatestRelease, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let response = client
        .get(GITHUB_LATEST_API)
        .header("User-Agent", "helm-desktop-updater")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("访问 GitHub 发布接口失败：{e}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // /releases/latest 会排除 pre-release 与 draft；仓库把所有版本都标为
        // pre-release 时这里会 404——回退到列表接口取最新一条（含 pre-release）
        let list = client
            .get(format!("{GITHUB_RELEASES}?per_page=1"))
            .header("User-Agent", "helm-desktop-updater")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("访问 GitHub 发布接口失败：{e}"))?;
        if !list.status().is_success() {
            return Err(format!("GitHub 发布接口返回 {}", list.status()));
        }
        let releases = list
            .json::<Vec<GitHubLatestRelease>>()
            .await
            .map_err(|e| format!("解析 GitHub 发布信息失败：{e}"))?;
        return releases.into_iter().next().ok_or_else(|| {
            "GitHub 仓库还没有发布版本；请先在发布页创建第一个 Release".to_string()
        });
    }
    if !response.status().is_success() {
        return Err(format!("GitHub 发布接口返回 {}", response.status()));
    }
    response
        .json::<GitHubLatestRelease>()
        .await
        .map_err(|e| format!("解析 GitHub 发布信息失败：{e}"))
}

/// 宽松版本比较：剥离 v 前缀后按数字逐段比较（如 0.5.0 vs 0.6.1）；
/// 非数字段按 0 处理，长度不足补 0。candidate > current 才算有新版。
fn version_gt(candidate: &str, current: &str) -> bool {
    fn parse(version: &str) -> Vec<u64> {
        version
            .trim()
            .trim_start_matches(|c| c == 'v' || c == 'V')
            .split(|c: char| c == '.' || c == '-' || c == '+')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    }
    let candidate = parse(candidate);
    let current = parse(current);
    let len = candidate.len().max(current.len());
    for index in 0..len {
        let candidate_part = candidate.get(index).copied().unwrap_or(0);
        let current_part = current.get(index).copied().unwrap_or(0);
        if candidate_part != current_part {
            return candidate_part > current_part;
        }
    }
    false
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

/// 检查更新：配置了签名发布源时走带验签的应用内更新链路；
/// 未配置时回退到 GitHub releases/latest 版本比对（只提示，不静默安装）。
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let feed = load_app_settings_from_store(&history_store)
        .map(|settings| settings.general.update_feed_url.trim().to_string())
        .unwrap_or_default();

    if feed.is_empty() {
        // 无发布源：GitHub 版本比对回退
        let latest = fetch_github_latest().await?;
        let available = version_gt(&latest.tag_name, &current_version);
        return Ok(UpdateCheckResult {
            current_version,
            available,
            version: if available {
                Some(latest.tag_name.trim_start_matches('v').to_string())
            } else {
                None
            },
            notes: if available { latest.body } else { None },
            source: "github",
            release_url: Some(latest.html_url),
        });
    }

    let updater = build_updater(&app, &feed)?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败：{e}"))?;

    Ok(match update {
        Some(update) => UpdateCheckResult {
            current_version,
            available: true,
            version: Some(update.version.clone()),
            notes: update.body.clone(),
            source: "feed",
            release_url: None,
        },
        None => UpdateCheckResult {
            current_version,
            available: false,
            version: None,
            notes: None,
            source: "feed",
            release_url: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gt_compares_numerically_with_v_prefix() {
        assert!(version_gt("v0.6.0", "0.5.0"));
        assert!(version_gt("0.5.1", "0.5.0"));
        assert!(version_gt("1.0", "0.9.9"));
        assert!(!version_gt("v0.5.0", "0.5.0"));
        assert!(!version_gt("0.4.9", "0.5.0"));
        assert!(!version_gt("0.5", "0.5.0"));
    }

    #[test]
    fn version_gt_tolerates_non_numeric_parts() {
        assert!(version_gt("0.6.0-beta.1", "0.5.0"));
        assert!(!version_gt("abc", "0.5.0"));
        assert!(version_gt("0.6.0", "abc"));
    }
}
