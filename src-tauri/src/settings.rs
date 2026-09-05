use crate::providers::{KeyringSecretStore, ProviderStore};
use crate::sessions::SessionHistoryStore;
use crate::subscription_profiles::SubscriptionProfileStore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(rename = "defaultDirectory")]
    pub default_directory: String,
    #[serde(rename = "reopenLastSession")]
    pub reopen_last_session: bool,
    #[serde(rename = "autoUpdateChannel")]
    pub auto_update_channel: String,
    #[serde(rename = "updateFeedUrl", default)]
    pub update_feed_url: String,
    /// 价格目录在后台更新；失败时始终回退到签名缓存或安装包内置目录。
    #[serde(rename = "pricingAutoUpdate", default = "default_true")]
    pub pricing_auto_update: bool,
    /// 国内主/备用镜像，一行一个完整 pricing-catalog.json URL。
    #[serde(rename = "pricingFeedUrls", default)]
    pub pricing_feed_urls: Vec<String>,
    /// warn：允许缺价模型运行；block：预算启用时缺价 fail-closed。
    #[serde(
        rename = "pricingUnknownPolicy",
        default = "default_pricing_unknown_policy"
    )]
    pub pricing_unknown_policy: String,
    #[serde(rename = "pricingMaxAgeDays", default = "default_pricing_max_age_days")]
    pub pricing_max_age_days: u32,
    /// 首启引导是否已完成/跳过（完成或显式跳过都置 true）
    #[serde(rename = "onboardingCompleted", default)]
    pub onboarding_completed: bool,
    /// 首轮结束后用绑定的 fast model 自动生成会话标题与摘要（P3-5）。
    /// 会把首轮对话内容发给用户自己绑定的服务商，属可关的外发行为。
    #[serde(rename = "autoTitleSessions", default = "default_true")]
    pub auto_title_sessions: bool,
    /// 生成式 UI 总开关（默认关闭，渲染能力后续接入）：开启才允许最终结果使用交互式可视化输出。
    #[serde(rename = "generativeUi", default)]
    pub generative_ui: bool,
    /// 点关闭按钮时最小化到托盘而不是退出（变更-12）：后台会话继续运行
    #[serde(rename = "closeToTray", default)]
    pub close_to_tray: bool,
    /// 轮次完成/出错时弹出系统通知
    #[serde(rename = "notifications", default)]
    pub notifications: Option<NotificationSettings>,
    /// 旧设置迁移输入；读取后迁入 Binding.fast_model，保存时清理。
    #[serde(rename = "assistantModelId", default)]
    pub assistant_model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_pricing_unknown_policy() -> String {
    "warn".to_string()
}

fn default_pricing_max_age_days() -> u32 {
    30
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionSettings {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub theme: String,
    #[serde(rename = "accentColor")]
    pub accent_color: AccentColor,
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
                default_directory: default_dir,
                reopen_last_session: true,
                auto_update_channel: "stable".to_string(),
                update_feed_url: String::new(),
                pricing_auto_update: true,
                pricing_feed_urls: Vec::new(),
                pricing_unknown_policy: default_pricing_unknown_policy(),
                pricing_max_age_days: default_pricing_max_age_days(),
                onboarding_completed: false,
                auto_title_sessions: true,
                generative_ui: false,
                close_to_tray: false,
                notifications: Some(NotificationSettings { enabled: true }),
                assistant_model_id: None,
            },
            engines: EngineSettings {
                default_engine: "claude-code".to_string(),
                claude_code: EngineConfig {
                    executable_path: String::new(),
                    version: String::new(),
                    detected: false,
                    permission_mode: Some("ask".to_string()),
                },
                codex: EngineConfig {
                    executable_path: String::new(),
                    version: String::new(),
                    detected: false,
                    permission_mode: None,
                },
            },
            permissions: PermissionSettings::default(),
            appearance: AppearanceSettings {
                theme: "light".to_string(),
                accent_color: AccentColor {
                    base: "oklch(52% 0.12 230)".to_string(),
                    hi: "oklch(46% 0.13 230)".to_string(),
                },
            },
            shortcuts: ShortcutSettings::default(),
        }
    }
}

pub fn load_app_settings_from_store(store: &SessionHistoryStore) -> Result<AppSettings, String> {
    let mut settings = store
        .get_json_setting(APP_SETTINGS_KEY)?
        .unwrap_or_else(AppSettings::default);
    normalize_general_settings(&mut settings.general);
    Ok(settings)
}

pub fn save_app_settings_to_store(
    store: &SessionHistoryStore,
    mut settings: AppSettings,
) -> Result<(), String> {
    normalize_general_settings(&mut settings.general);
    settings.general.assistant_model_id = None;
    store.set_json_setting(APP_SETTINGS_KEY, &settings)
}

fn normalize_general_settings(settings: &mut GeneralSettings) {
    settings.pricing_feed_urls = settings
        .pricing_feed_urls
        .iter()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .collect();
    settings.pricing_feed_urls.dedup();
    if !matches!(settings.pricing_unknown_policy.as_str(), "warn" | "block") {
        settings.pricing_unknown_policy = default_pricing_unknown_policy();
    }
    settings.pricing_max_age_days = settings.pricing_max_age_days.clamp(1, 3650);
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
    provider_store: State<'_, ProviderStore<KeyringSecretStore>>,
    settings: AppSettings,
) -> Result<(), String> {
    provider_store
        .migrate_legacy_assistant_model(settings.general.assistant_model_id.as_deref())?;
    save_app_settings_to_store(&history_store, settings)
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

    // Windows：npm 全局安装的 claude/codex 是 .cmd 垫片，CreateProcess 不能直接执行，
    // 必须经 cmd /C 中转（与 providers.rs build_version_command 同款处理），否则版本恒为 unknown。
    #[cfg(windows)]
    let version = {
        use std::os::windows::process::CommandExt;
        let mut version_cmd = std::process::Command::new("cmd");
        version_cmd.arg("/C").arg(&path).arg("--version");
        version_cmd.creation_flags(0x0800_0000);
        match version_cmd.output() {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .trim()
                .to_string(),
            _ => "unknown".to_string(),
        }
    };
    #[cfg(not(windows))]
    let version = {
        let mut version_cmd = std::process::Command::new(&path);
        version_cmd.arg("--version");
        match version_cmd.output() {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("unknown")
                .trim()
                .to_string(),
            _ => "unknown".to_string(),
        }
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

/// CLI 登录态探测结果：只信任官方 CLI 状态命令，判断不了绝不臆断。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliLoginState {
    pub state: String,
    pub auth_method: Option<String>,
    pub account_label: Option<String>,
    pub plan: Option<String>,
    pub detail: String,
}

fn login_state(state: &str, auth_method: &str, detail: &str) -> CliLoginState {
    CliLoginState {
        state: state.to_string(),
        auth_method: Some(auth_method.to_string()),
        account_label: None,
        plan: None,
        detail: detail.to_string(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeAuthStatus {
    logged_in: bool,
    auth_method: Option<String>,
    #[serde(default)]
    account_label: Option<String>,
    #[serde(default)]
    plan: Option<String>,
}

fn parse_auth_status(engine: &str, success: bool, stdout: &[u8], stderr: &[u8]) -> CliLoginState {
    if !success {
        let error = format!(
            "{} {}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        )
        .to_ascii_lowercase();
        return if error.contains("expired") || error.contains("过期") {
            login_state("expired", "unknown", "CLI 登录已失效，请重新登录")
        } else {
            login_state("missing", "unknown", "CLI 当前未登录")
        };
    }
    match engine {
        "claude-code" => {
            let Ok(status) = serde_json::from_slice::<ClaudeAuthStatus>(stdout) else {
                return login_state("unknown", "unknown", "无法解析 Claude Code 登录状态");
            };
            if !status.logged_in {
                return login_state("missing", "unknown", "Claude Code 当前未登录");
            }
            let method = status.auth_method.unwrap_or_default().to_ascii_lowercase();
            let auth_method = if method.contains("oauth") {
                "subscription"
            } else if method.contains("api") {
                "apikey"
            } else {
                "unknown"
            };
            let detail = match auth_method {
                "subscription" => "Claude Code 已通过官方订阅登录",
                "apikey" => "Claude Code 当前使用 API Key 登录",
                _ => "Claude Code 已登录，但无法识别认证方式",
            };
            CliLoginState {
                state: "ok".to_string(),
                auth_method: Some(auth_method.to_string()),
                account_label: status.account_label,
                plan: status.plan,
                detail: detail.to_string(),
            }
        }
        "codex" => {
            // Codex versions differ on whether `login status` writes its successful
            // status to stdout or stderr. Both streams are authoritative only after
            // the command exits successfully; never persist or surface the raw text.
            let output = format!(
                "{} {}",
                String::from_utf8_lossy(stdout),
                String::from_utf8_lossy(stderr)
            )
            .to_ascii_lowercase();
            let (auth_method, detail) = if output.contains("chatgpt") {
                ("subscription", "Codex 已通过 ChatGPT 订阅登录")
            } else if output.contains("api key") {
                ("apikey", "Codex 当前使用 API Key 登录")
            } else {
                ("unknown", "Codex 已登录，但无法识别认证方式")
            };
            login_state("ok", auth_method, detail)
        }
        _ => login_state("unknown", "unknown", "未知引擎，无法检测登录状态"),
    }
}

async fn run_auth_status(
    profiles: &SubscriptionProfileStore,
    engine: &str,
) -> Result<CliLoginState, String> {
    let (bin, args): (&str, &[&str]) = match engine {
        "claude-code" => ("claude", &["auth", "status"]),
        "codex" => ("codex", &["login", "status"]),
        other => return Err(format!("未知引擎：{other}")),
    };
    let mut command = match engine {
        "claude-code" => crate::adapter::build_command(bin),
        "codex" => crate::adapter::build_codex_command(bin),
        _ => unreachable!("engine was validated above"),
    };
    profiles.configure_command(&mut command, engine)?;
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = match tokio::time::timeout(Duration::from_secs(10), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return Ok(login_state(
                "unknown",
                "unknown",
                &format!("无法执行 {bin} 登录状态命令：{error}"),
            ));
        }
        Err(_) => {
            return Ok(login_state(
                "unknown",
                "unknown",
                &format!("{bin} 登录状态检测超时"),
            ));
        }
    };
    Ok(parse_auth_status(
        engine,
        output.status.success(),
        &output.stdout,
        &output.stderr,
    ))
}

fn validate_subscription_login(state: &CliLoginState) -> Result<(), String> {
    if state.state == "ok" && state.auth_method.as_deref() == Some("subscription") {
        return Ok(());
    }
    Err(match (state.state.as_str(), state.auth_method.as_deref()) {
        ("ok", Some("apikey")) => {
            "[subscription_login_required] 当前 CLI 使用 API Key 登录，不能用于官方订阅绑定"
                .to_string()
        }
        ("missing", _) => {
            "[subscription_login_required] 当前 CLI 未登录官方订阅，请先完成登录".to_string()
        }
        ("expired", _) => {
            "[subscription_login_required] 当前 CLI 订阅登录已失效，请重新登录".to_string()
        }
        _ => "[subscription_login_unavailable] 无法确认当前 CLI 的官方订阅登录状态，请重新检测"
            .to_string(),
    })
}

pub(crate) async fn ensure_subscription_login(
    profiles: &SubscriptionProfileStore,
    engine: &str,
) -> Result<(), String> {
    let state = run_auth_status(profiles, engine).await?;
    validate_subscription_login(&state)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthAction {
    Login,
    Logout,
}

fn auth_command_spec(
    engine: &str,
    action: AuthAction,
) -> Result<(&'static str, Vec<&'static str>), String> {
    match (engine, action) {
        ("claude-code", AuthAction::Login) => Ok(("claude", vec!["auth", "login"])),
        ("claude-code", AuthAction::Logout) => Ok(("claude", vec!["auth", "logout"])),
        ("codex", AuthAction::Login) => Ok(("codex", vec!["login"])),
        ("codex", AuthAction::Logout) => Ok(("codex", vec!["logout"])),
        (other, _) => Err(format!("未知引擎：{other}")),
    }
}

async fn run_auth_lifecycle(
    profiles: &SubscriptionProfileStore,
    engine: &str,
    action: AuthAction,
) -> Result<(), String> {
    let (bin, args) = auth_command_spec(engine, action)?;
    let mut command = match engine {
        "claude-code" => crate::adapter::build_command(bin),
        "codex" => crate::adapter::build_codex_command(bin),
        _ => unreachable!("engine was validated above"),
    };
    profiles.configure_command(&mut command, engine)?;
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let timeout = match action {
        AuthAction::Login => Duration::from_secs(300),
        AuthAction::Logout => Duration::from_secs(30),
    };
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| match action {
            AuthAction::Login => "等待官方账号登录超时，请重试".to_string(),
            AuthAction::Logout => "退出登录超时，请重试".to_string(),
        })?
        .map_err(|error| format!("无法执行 {bin}：{error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(match action {
            AuthAction::Login => "官方账号登录未完成或已取消".to_string(),
            AuthAction::Logout => "退出登录失败，请重试".to_string(),
        })
    }
}

#[tauri::command]
pub async fn login_cli_account(
    profiles: State<'_, SubscriptionProfileStore>,
    engine: String,
) -> Result<CliLoginState, String> {
    run_auth_lifecycle(&profiles, &engine, AuthAction::Login).await?;
    run_auth_status(&profiles, &engine).await
}

#[tauri::command]
pub async fn logout_cli_account(
    profiles: State<'_, SubscriptionProfileStore>,
    engine: String,
) -> Result<CliLoginState, String> {
    run_auth_lifecycle(&profiles, &engine, AuthAction::Logout).await?;
    run_auth_status(&profiles, &engine).await
}

/// 检测某个引擎的 Helm-owned 订阅 Profile 登录态。
#[tauri::command]
pub async fn detect_cli_login(
    profiles: State<'_, SubscriptionProfileStore>,
    engine: String,
) -> Result<CliLoginState, String> {
    run_auth_status(&profiles, &engine).await
}

async fn engine_login_state(
    profiles: &SubscriptionProfileStore,
    executable: &str,
) -> CliLoginState {
    match executable {
        "claude" => run_auth_status(profiles, "claude-code")
            .await
            .unwrap_or_else(|error| login_state("unknown", "unknown", &error)),
        "codex" => run_auth_status(profiles, "codex")
            .await
            .unwrap_or_else(|error| login_state("unknown", "unknown", &error)),
        _ => login_state("unknown", "unknown", "未知引擎，无法检测登录状态"),
    }
}

async fn engine_readiness(
    profiles: &SubscriptionProfileStore,
    executable: &str,
) -> EngineReadiness {
    let login = engine_login_state(profiles, executable).await;
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

/// 生成就绪度报告，并把检测结果回写 ProviderStore 的引擎状态
/// （消灭服务商页首启时的"未检测"初始态）。
#[tauri::command]
pub async fn get_readiness_report(
    history_store: State<'_, SessionHistoryStore>,
    config_store: State<'_, crate::providers::ProviderStore<crate::providers::KeyringSecretStore>>,
    profiles: State<'_, SubscriptionProfileStore>,
) -> Result<ReadinessReport, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    let claude = engine_readiness(&profiles, "claude").await;
    let codex = engine_readiness(&profiles, "codex").await;

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

#[cfg(test)]
mod permission_settings_compat_tests {
    use super::AppSettings;

    #[test]
    fn legacy_permission_and_sandbox_fields_are_read_but_removed_on_serialize() {
        let mut legacy = serde_json::to_value(AppSettings::default()).unwrap();
        legacy["general"]["confirmBeforeCommand"] = serde_json::json!(false);
        legacy["engines"]["codex"]["sandbox"] = serde_json::json!("full");
        legacy["permissions"] = serde_json::json!({
            "preset": "autonomous",
            "autonomousNetwork": true,
            "readFiles": "deny",
            "editFiles": "allow",
            "runCommands": "allow",
            "fetchUrls": "allow",
            "mcpTools": "allow",
            "commandAllowlist": ["git status"],
            "commandProfiles": ["git-status"]
        });

        let settings: AppSettings = serde_json::from_value(legacy).unwrap();
        let saved = serde_json::to_value(settings).unwrap();

        assert_eq!(saved["permissions"], serde_json::json!({}));
        assert!(saved["general"].get("confirmBeforeCommand").is_none());
        assert!(saved["engines"]["codex"].get("sandbox").is_none());
    }
}

#[cfg(test)]
mod auth_status_tests {
    use super::*;

    #[test]
    fn legacy_worktree_settings_are_ignored_and_not_serialized_again() {
        let mut legacy = serde_json::to_value(AppSettings::default()).unwrap();
        legacy["worktree"] = serde_json::json!({
            "enabled": true,
            "root": "D:/legacy-worktrees",
            "setupScript": "npm install"
        });

        let settings: AppSettings = serde_json::from_value(legacy).unwrap();
        let saved = serde_json::to_value(settings).unwrap();

        assert!(saved.get("worktree").is_none());
    }

    #[test]
    fn parses_claude_subscription_status_json() {
        let state = parse_auth_status(
            "claude-code",
            true,
            br#"{"loggedIn":true,"authMethod":"oauth_token","apiProvider":"firstParty"}"#,
            b"",
        );
        assert_eq!(state.state, "ok");
        assert_eq!(state.auth_method.as_deref(), Some("subscription"));
        assert_eq!(state.detail, "Claude Code 已通过官方订阅登录");
    }

    #[test]
    fn parses_claude_api_key_status_json() {
        let state = parse_auth_status(
            "claude-code",
            true,
            br#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#,
            b"",
        );
        assert_eq!(state.state, "ok");
        assert_eq!(state.auth_method.as_deref(), Some("apikey"));
    }

    #[test]
    fn parses_codex_chatgpt_login_status() {
        let state = parse_auth_status("codex", true, b"Logged in using ChatGPT\n", b"");
        assert_eq!(state.state, "ok");
        assert_eq!(state.auth_method.as_deref(), Some("subscription"));
        assert!(!state.detail.contains("token"));
    }

    #[test]
    fn parses_codex_chatgpt_login_status_from_stderr() {
        let state = parse_auth_status("codex", true, b"", b"Logged in using ChatGPT\n");
        assert_eq!(state.state, "ok");
        assert_eq!(state.auth_method.as_deref(), Some("subscription"));
        assert_eq!(state.detail, "Codex 已通过 ChatGPT 订阅登录");
    }

    #[test]
    fn parses_codex_api_key_login_status_without_exposing_key() {
        let state = parse_auth_status(
            "codex",
            true,
            b"Logged in using an API key - sk-sensitive-value\n",
            b"",
        );
        assert_eq!(state.state, "ok");
        assert_eq!(state.auth_method.as_deref(), Some("apikey"));
        assert_eq!(state.detail, "Codex 当前使用 API Key 登录");
        assert!(!state.detail.contains("sk-sensitive-value"));
    }

    #[test]
    fn failed_status_exit_is_missing_or_expired_not_ok() {
        let missing = parse_auth_status("codex", false, b"", b"Not logged in");
        assert_eq!(missing.state, "missing");
        let expired = parse_auth_status("claude-code", false, b"", b"OAuth token expired");
        assert_eq!(expired.state, "expired");
    }

    #[test]
    fn malformed_successful_status_is_unknown() {
        let state = parse_auth_status("claude-code", true, b"not-json", b"");
        assert_eq!(state.state, "unknown");
        assert_eq!(state.auth_method.as_deref(), Some("unknown"));
    }

    #[test]
    fn subscription_binding_requires_authoritative_subscription_login() {
        assert!(validate_subscription_login(&login_state(
            "ok",
            "subscription",
            "official subscription"
        ))
        .is_ok());

        for state in [
            login_state("ok", "apikey", "api key"),
            login_state("missing", "unknown", "missing"),
            login_state("expired", "unknown", "expired"),
            login_state("unknown", "unknown", "probe failed"),
        ] {
            let error = validate_subscription_login(&state).unwrap_err();
            assert!(error.starts_with("[subscription_login_"));
            assert!(!error.contains(&state.detail));
        }
    }

    #[test]
    fn cli_auth_command_specs_are_engine_specific() {
        assert_eq!(
            auth_command_spec("claude-code", AuthAction::Login).unwrap(),
            ("claude", vec!["auth", "login"])
        );
        assert_eq!(
            auth_command_spec("claude-code", AuthAction::Logout).unwrap(),
            ("claude", vec!["auth", "logout"])
        );
        assert_eq!(
            auth_command_spec("codex", AuthAction::Login).unwrap(),
            ("codex", vec!["login"])
        );
        assert_eq!(
            auth_command_spec("codex", AuthAction::Logout).unwrap(),
            ("codex", vec!["logout"])
        );
    }
}
