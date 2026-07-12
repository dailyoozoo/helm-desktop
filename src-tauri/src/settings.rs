use crate::sessions::SessionHistoryStore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;

const APP_SETTINGS_KEY: &str = "app_settings";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub general: GeneralSettings,
    pub engines: EngineSettings,
    pub permissions: PermissionSettings,
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub shortcuts: ShortcutSettings,
    /// 并行会话的 worktree 隔离（P3-3）；带 default 以兼容旧设置数据
    #[serde(default)]
    pub worktree: WorktreeSettings,
}

/// worktree 隔离配置：可关闭、位置可配、支持 setup 脚本（P3-3 避坑清单）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSettings {
    pub enabled: bool,
    /// 空 = 默认放在仓库旁边的 `<仓库名>-worktrees/`
    #[serde(default)]
    pub root: String,
    /// worktree 创建后在其中执行的初始化命令（如 npm install）；空 = 不执行
    #[serde(default)]
    pub setup_script: String,
}

impl Default for WorktreeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            root: String::new(),
            setup_script: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(rename = "workspaceName")]
    pub workspace_name: String,
    #[serde(rename = "defaultDirectory")]
    pub default_directory: String,
    #[serde(rename = "reopenLastSession")]
    pub reopen_last_session: bool,
    #[serde(rename = "confirmBeforeCommand")]
    pub confirm_before_command: bool,
    #[serde(rename = "anonymousAnalytics")]
    pub anonymous_analytics: bool,
    #[serde(rename = "autoUpdateChannel")]
    pub auto_update_channel: String,
    #[serde(rename = "updateFeedUrl", default)]
    pub update_feed_url: String,
    /// 首启引导是否已完成/跳过（完成或显式跳过都置 true）
    #[serde(rename = "onboardingCompleted", default)]
    pub onboarding_completed: bool,
    /// 首轮结束后用绑定的 fast model 自动生成会话标题与摘要（P3-5）。
    /// 会把首轮对话内容发给用户自己绑定的服务商，属可关的外发行为。
    #[serde(rename = "autoTitleSessions", default = "default_true")]
    pub auto_title_sessions: bool,
    /// 点关闭按钮时最小化到托盘而不是退出（变更-12）：后台会话继续运行
    #[serde(rename = "closeToTray", default)]
    pub close_to_tray: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSettings {
    #[serde(rename = "defaultEngine")]
    pub default_engine: String,
    #[serde(rename = "claudeCode")]
    pub claude_code: EngineConfig,
    pub codex: EngineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(rename = "executablePath")]
    pub executable_path: String,
    pub version: String,
    pub detected: bool,
    #[serde(rename = "permissionMode", skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSettings {
    #[serde(rename = "readFiles")]
    pub read_files: String,
    #[serde(rename = "editFiles")]
    pub edit_files: String,
    #[serde(rename = "runCommands")]
    pub run_commands: String,
    #[serde(rename = "fetchUrls")]
    pub fetch_urls: String,
    #[serde(rename = "mcpTools")]
    pub mcp_tools: String,
    #[serde(rename = "commandAllowlist")]
    pub command_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,
    #[serde(rename = "accentColor")]
    pub accent_color: AccentColor,
    #[serde(rename = "uiDensity")]
    pub ui_density: String,
    #[serde(rename = "monospaceFont")]
    pub monospace_font: String,
    #[serde(rename = "reduceMotion")]
    pub reduce_motion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccentColor {
    pub base: String,
    pub hi: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutSettings {
    pub command_palette: String,
    pub new_session: String,
    pub toggle_context: String,
    pub cycle_engine: String,
    pub navigation_prefix: String,
    pub home: String,
    pub workspace: String,
    pub providers: String,
    pub sessions: String,
    pub extensions: String,
    pub usage: String,
    pub settings: String,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            command_palette: "Ctrl+K".to_string(),
            new_session: "Ctrl+N".to_string(),
            toggle_context: "Ctrl+.".to_string(),
            cycle_engine: "Ctrl+E".to_string(),
            navigation_prefix: "G".to_string(),
            home: "H".to_string(),
            workspace: "W".to_string(),
            providers: "P".to_string(),
            sessions: "S".to_string(),
            extensions: "X".to_string(),
            usage: "U".to_string(),
            settings: ",".to_string(),
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        let default_dir = if cfg!(windows) {
            dirs::document_dir()
                .and_then(|d| d.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "C:\\Users\\Public\\Documents".to_string())
        } else {
            dirs::home_dir()
                .map(|d| d.join("code"))
                .and_then(|d| d.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "~/code".to_string())
        };

        Self {
            general: GeneralSettings {
                workspace_name: "我的工作区".to_string(),
                default_directory: default_dir,
                reopen_last_session: true,
                confirm_before_command: true,
                anonymous_analytics: false,
                auto_update_channel: "stable".to_string(),
                update_feed_url: String::new(),
                onboarding_completed: false,
                auto_title_sessions: true,
                close_to_tray: false,
            },
            engines: EngineSettings {
                default_engine: "claude-code".to_string(),
                claude_code: EngineConfig {
                    executable_path: String::new(),
                    version: String::new(),
                    detected: false,
                    permission_mode: Some("ask".to_string()),
                    sandbox: None,
                },
                codex: EngineConfig {
                    executable_path: String::new(),
                    version: String::new(),
                    detected: false,
                    permission_mode: None,
                    sandbox: Some("workspace".to_string()),
                },
            },
            permissions: PermissionSettings {
                read_files: "allow".to_string(),
                edit_files: "allow".to_string(),
                run_commands: "ask".to_string(),
                fetch_urls: "deny".to_string(),
                mcp_tools: "ask".to_string(),
                command_allowlist: vec![
                    "git status".to_string(),
                    "git diff".to_string(),
                    "pnpm test *".to_string(),
                    "pnpm lint".to_string(),
                    "ls *".to_string(),
                ],
            },
            appearance: AppearanceSettings {
                theme: "light".to_string(),
                accent_color: AccentColor {
                    base: "oklch(55% 0.2 264)".to_string(),
                    hi: "oklch(49% 0.21 264)".to_string(),
                },
                ui_density: "comfortable".to_string(),
                monospace_font: "JetBrains Mono".to_string(),
                reduce_motion: false,
            },
            shortcuts: ShortcutSettings::default(),
            worktree: WorktreeSettings::default(),
        }
    }
}

pub fn load_app_settings_from_store(store: &SessionHistoryStore) -> Result<AppSettings, String> {
    Ok(store
        .get_json_setting(APP_SETTINGS_KEY)?
        .unwrap_or_else(AppSettings::default))
}

pub fn save_app_settings_to_store(
    store: &SessionHistoryStore,
    settings: AppSettings,
) -> Result<(), String> {
    store.set_json_setting(APP_SETTINGS_KEY, &settings)
}

pub fn export_app_settings_to_path(store: &SessionHistoryStore, path: &Path) -> Result<(), String> {
    let settings = load_app_settings_from_store(store)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败：{e}"))?;
    }
    let content =
        serde_json::to_string_pretty(&settings).map_err(|e| format!("序列化设置失败：{e}"))?;
    std::fs::write(path, content).map_err(|e| format!("写入设置导出文件失败：{e}"))
}

pub fn import_app_settings_from_path(
    store: &SessionHistoryStore,
    path: &Path,
) -> Result<AppSettings, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("读取设置文件失败：{e}"))?;
    let settings: AppSettings =
        serde_json::from_str(&content).map_err(|e| format!("解析设置文件失败：{e}"))?;
    save_app_settings_to_store(store, settings.clone())?;
    Ok(settings)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current_version: String,
    pub channel: String,
    pub can_check: bool,
    pub message: String,
}

pub fn update_status_from_settings(settings: &AppSettings) -> UpdateStatus {
    let feed_url = settings.general.update_feed_url.trim();
    UpdateStatus {
        current_version: env!("CARGO_PKG_VERSION").to_string(),
        channel: settings.general.auto_update_channel.clone(),
        can_check: !feed_url.is_empty(),
        message: if feed_url.is_empty() {
            "未配置自动更新发布源；当前仅保存更新通道偏好。".to_string()
        } else {
            format!("已配置自动更新发布源：{feed_url}")
        },
    }
}

#[tauri::command]
pub fn load_app_settings(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<AppSettings, String> {
    load_app_settings_from_store(&history_store)
}

#[tauri::command]
pub fn save_app_settings(
    history_store: State<'_, SessionHistoryStore>,
    settings: AppSettings,
) -> Result<(), String> {
    save_app_settings_to_store(&history_store, settings)
}

#[tauri::command]
pub fn export_app_settings(
    app: tauri::AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let Some(path) = app
        .dialog()
        .file()
        .set_title("导出 Helm 设置")
        .add_filter("JSON", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = match path {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("不支持的导出路径：{url}")),
    };
    export_app_settings_to_path(&history_store, &path)?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[tauri::command]
pub fn import_app_settings(
    app: tauri::AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Option<AppSettings>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let Some(path) = app
        .dialog()
        .file()
        .set_title("导入 Helm 设置")
        .add_filter("JSON", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = match path {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("不支持的导入路径：{url}")),
    };
    import_app_settings_from_path(&history_store, &path).map(Some)
}

#[tauri::command]
pub fn get_update_status(
    history_store: State<'_, SessionHistoryStore>,
) -> Result<UpdateStatus, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    Ok(update_status_from_settings(&settings))
}

#[derive(Debug, Serialize)]
pub struct EngineDetectionResult {
    pub path: String,
    pub version: String,
}

/// 按可执行文件名做真实检测：where/which 定位 + `--version`。
/// 供手动检测命令与启动时的就绪度检查共用。
fn detect_engine_binary(executable: &str) -> Result<EngineDetectionResult, String> {
    #[cfg(windows)]
    let which_cmd = "where";
    #[cfg(not(windows))]
    let which_cmd = "which";

    let mut locate = std::process::Command::new(which_cmd);
    locate.arg(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        locate.creation_flags(0x0800_0000);
    }
    let output = locate
        .output()
        .map_err(|e| format!("执行 {which_cmd} 失败：{e}"))?;

    if !output.status.success() {
        return Err(format!("在 PATH 中找不到 {executable}，可能尚未安装"));
    }

    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    if path.is_empty() {
        return Err(format!("在 PATH 中找不到 {executable}，可能尚未安装"));
    }

    let mut version_cmd = std::process::Command::new(&path);
    version_cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        version_cmd.creation_flags(0x0800_0000);
    }
    let version = match version_cmd.output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .trim()
            .to_string(),
        _ => "unknown".to_string(),
    };

    Ok(EngineDetectionResult { path, version })
}

fn engine_executable(engine: &str) -> Result<&'static str, String> {
    match engine {
        "claude-code" => Ok("claude"),
        "codex" => Ok("codex"),
        _ => Err(format!("未知引擎：{engine}")),
    }
}

#[tauri::command]
pub fn detect_cli_engine(engine: String) -> Result<EngineDetectionResult, String> {
    detect_engine_binary(engine_executable(&engine)?)
}

/// 单个引擎的就绪度。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineReadiness {
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub error: Option<String>,
    /// CLI 自身登录态（订阅登录一等公民，P3-1）
    pub login: CliLoginState,
}

/// CLI 登录态探测结果：ok / missing / unknown（判断不了绝不臆断）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliLoginState {
    pub state: String,
    pub detail: String,
}

fn claude_login_state() -> CliLoginState {
    let Some(home) = dirs::home_dir() else {
        return CliLoginState {
            state: "unknown".to_string(),
            detail: "无法定位用户主目录".to_string(),
        };
    };
    let cred = home.join(".claude").join(".credentials.json");
    if cred.is_file() {
        CliLoginState {
            state: "ok".to_string(),
            detail: "检测到 Claude Code 订阅登录凭证（~/.claude/.credentials.json）".to_string(),
        }
    } else if cfg!(target_os = "macos") {
        // macOS 凭证存系统钥匙串，文件缺失不代表未登录
        CliLoginState {
            state: "unknown".to_string(),
            detail: "macOS 上凭证存于系统钥匙串，无法通过文件判断；可在终端运行 claude 验证"
                .to_string(),
        }
    } else {
        CliLoginState {
            state: "missing".to_string(),
            detail: "未检测到订阅登录凭证；请在终端运行 claude 并完成登录".to_string(),
        }
    }
}

fn codex_login_state() -> CliLoginState {
    let Some(home) = dirs::home_dir() else {
        return CliLoginState {
            state: "unknown".to_string(),
            detail: "无法定位用户主目录".to_string(),
        };
    };
    let auth = home.join(".codex").join("auth.json");
    if auth.is_file() {
        CliLoginState {
            state: "ok".to_string(),
            detail: "检测到 Codex 登录凭证（~/.codex/auth.json）".to_string(),
        }
    } else {
        CliLoginState {
            state: "missing".to_string(),
            detail: "未检测到 ~/.codex/auth.json；请在终端运行 codex login 完成登录".to_string(),
        }
    }
}

/// 检测某个引擎 CLI 的本机登录态（订阅登录模式的「检测登录态」按钮用）
#[tauri::command]
pub fn detect_cli_login(engine: String) -> Result<CliLoginState, String> {
    match engine.as_str() {
        "claude-code" => Ok(claude_login_state()),
        "codex" => Ok(codex_login_state()),
        other => Err(format!("未知引擎：{other}")),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CwdReadiness {
    pub configured: bool,
    pub exists: bool,
    pub path: String,
}

/// 冷启动就绪度报告：首启向导与发送前置校验共用（可靠性检查 §4.1）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessReport {
    pub claude_code: EngineReadiness,
    pub codex: EngineReadiness,
    pub has_provider: bool,
    pub has_ready_provider: bool,
    pub default_engine: String,
    /// 已配置生效绑定的引擎 id 列表
    pub bound_engines: Vec<String>,
    pub cwd: CwdReadiness,
}

fn engine_readiness(executable: &str) -> EngineReadiness {
    let login = match executable {
        "claude" => claude_login_state(),
        "codex" => codex_login_state(),
        _ => CliLoginState {
            state: "unknown".to_string(),
            detail: String::new(),
        },
    };
    match detect_engine_binary(executable) {
        Ok(result) => EngineReadiness {
            installed: true,
            path: Some(result.path),
            version: Some(result.version),
            error: None,
            login,
        },
        Err(error) => EngineReadiness {
            installed: false,
            path: None,
            version: None,
            error: Some(error),
            login,
        },
    }
}

/// 生成就绪度报告，并把检测结果回写 ProviderStore 的引擎状态
/// （消灭服务商页首启时的"未检测"初始态）。
#[tauri::command]
pub fn get_readiness_report(
    history_store: State<'_, SessionHistoryStore>,
    config_store: State<'_, crate::providers::ProviderStore<crate::providers::KeyringSecretStore>>,
) -> Result<ReadinessReport, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    let claude = engine_readiness("claude");
    let codex = engine_readiness("codex");

    let mut config = config_store.load()?;
    let mut changed = false;
    for engine in config.engines.iter_mut() {
        let readiness = match engine.id.as_str() {
            "claude-code" => &claude,
            "codex" => &codex,
            _ => continue,
        };
        let next_status = if readiness.installed {
            crate::providers::EngineStatus::Ready
        } else {
            crate::providers::EngineStatus::Missing
        };
        if engine.status != next_status || engine.version != readiness.version {
            engine.status = next_status;
            engine.version = readiness.version.clone();
            if let Some(path) = &readiness.path {
                engine.bin = path.clone();
            }
            changed = true;
        }
    }
    if changed {
        config_store.save(&config)?;
    }

    let cwd_path = settings.general.default_directory.trim().to_string();
    Ok(ReadinessReport {
        claude_code: claude,
        codex,
        has_provider: !config.providers.is_empty(),
        has_ready_provider: config.providers.iter().any(|provider| provider.ready),
        default_engine: config.default_engine.clone(),
        bound_engines: config
            .bindings
            .iter()
            .map(|binding| binding.engine_id.clone())
            .collect(),
        cwd: CwdReadiness {
            configured: !cwd_path.is_empty(),
            exists: !cwd_path.is_empty() && Path::new(&cwd_path).is_dir(),
            path: cwd_path,
        },
    })
}

#[tauri::command]
pub async fn select_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let result = app
        .dialog()
        .file()
        .set_title("选择工作目录")
        .blocking_pick_folder();

    match result {
        Some(folder) => {
            let path = match folder {
                FilePath::Path(p) => p.to_string_lossy().to_string(),
                FilePath::Url(u) => u.to_string(),
            };
            Ok(Some(path))
        }
        None => Ok(None),
    }
}
