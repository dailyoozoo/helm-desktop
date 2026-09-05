//! 设置页「关于」切片（S8）后端：平台信息、日志目录、脱敏诊断包、历史对话导入。
//!
//! 契约边界（对齐 docs/全量差异审计报告-2026-08-22.md API-09）：
//! - 诊断包只收集本地事实（版本/平台/聚合统计/设置），密钥、providers 配置、
//!   环境变量、权限审计明细不收集；自由文本统一过 redact_text。
//! - 历史导入只读取本机 Claude Code / Codex 记录文件或用户显式选择的 JSONL，
//!   内容只写入本地 SQLite（message.turn_id 保持 NULL，不伪造 Turn/Usage），
//!   不发起任何网络请求。
//! - 失败语义全部显式：文件过大、无可导入消息、超消息上限都返回 Err，不静默截断。

use crate::protocol::EngineId;
use crate::redaction::redact_text;
use crate::sessions::{ImportedHistoryMessage, NewSessionRecord, SessionHistoryStore};
use crate::util::now_millis;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

/// 单个历史记录文件大小上限（20 MiB）：超过直接报错，不静默截断。
pub const MAX_HISTORY_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// 单次导入消息条数上限：超过显式失败，提示用户分批导入。
pub const MAX_IMPORT_MESSAGES: usize = 5000;
/// 扫描列表最多返回的条数；超出部分用 total_found/truncated 表达，不静默丢弃。
pub const MAX_SCAN_ENTRIES: usize = 200;
/// 列表预览截断长度（仅本地 UI 显示，不进诊断包）。
const PREVIEW_MAX_CHARS: usize = 160;

// ─── 平台信息 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    pub os_name: String,
    /// 仅 Windows 提供 RtlGetVersion 的真实内核版本号（如 10.0.22631）；其他平台为 null。
    pub os_version: Option<String>,
    pub arch: String,
    pub app_version: String,
    pub tauri_version: String,
    pub webview_version: String,
}

#[tauri::command]
pub fn get_platform_info() -> PlatformInfo {
    let os_name = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        std::env::consts::OS
    };
    PlatformInfo {
        os_name: os_name.to_string(),
        os_version: windows_version(),
        arch: std::env::consts::ARCH.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        tauri_version: tauri::VERSION.to_string(),
        webview_version: tauri::webview_version().unwrap_or_else(|_| "unknown".to_string()),
    }
}

/// RtlGetVersion 返回未被兼容 shim 篡改的真实内核版本；失败返回 None。
#[cfg(windows)]
fn windows_version() -> Option<String> {
    #[repr(C)]
    struct OsVersionInfoW {
        dw_os_version_info_size: u32,
        dw_major_version: u32,
        dw_minor_version: u32,
        dw_build_number: u32,
        dw_platform_id: u32,
        sz_csd_version: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        dw_os_version_info_size: std::mem::size_of::<OsVersionInfoW>() as u32,
        dw_major_version: 0,
        dw_minor_version: 0,
        dw_build_number: 0,
        dw_platform_id: 0,
        sz_csd_version: [0; 128],
    };
    let status = unsafe { RtlGetVersion(&mut info) };
    if status == 0 {
        Some(format!(
            "{}.{}.{}",
            info.dw_major_version, info.dw_minor_version, info.dw_build_number
        ))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn windows_version() -> Option<String> {
    None
}

// ─── 日志目录 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastDiagnosticsExport {
    pub path: String,
    pub exported_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogDirInfo {
    pub path: String,
    pub file_count: usize,
    /// 最近一次诊断包导出记录（存在时）；内容只含导出路径与时间。
    pub last_diagnostics_export: Option<LastDiagnosticsExport>,
}

fn log_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("获取配置目录失败：{e}"))?;
    Ok(config_dir.join("logs"))
}

fn read_last_diagnostics_marker(dir: &Path) -> Option<LastDiagnosticsExport> {
    let content = std::fs::read_to_string(dir.join("last-diagnostics-export.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    Some(LastDiagnosticsExport {
        path: value.get("path")?.as_str()?.to_string(),
        exported_at: value.get("exportedAt")?.as_str()?.to_string(),
    })
}

#[tauri::command]
pub fn get_log_dir_info(app: tauri::AppHandle) -> Result<LogDirInfo, String> {
    let dir = log_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
    let file_count = std::fs::read_dir(&dir)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    Ok(LogDirInfo {
        path: dir.to_string_lossy().to_string(),
        file_count,
        last_diagnostics_export: read_last_diagnostics_marker(&dir),
    })
}

// ─── 时间工具（无 chrono 依赖） ─────────────────────────────────────────────

fn iso_now() -> String {
    let secs = now_millis() / 1000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn chrono_file_stamp() -> String {
    let secs = now_millis() / 1000;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant 的 civil_from_days：Unix 天数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Howard Hinnant 的 days_from_civil：(年, 月, 日) → Unix 天数。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

// ─── 诊断包 ─────────────────────────────────────────────────────────────────

/// 收集诊断包内容（纯函数便于测试）：只含本地事实与聚合统计。
fn build_diagnostics_value(
    history_store: &SessionHistoryStore,
    platform: &PlatformInfo,
    generated_at: &str,
) -> Result<serde_json::Value, String> {
    let settings = crate::settings::load_app_settings_from_store(history_store)?;
    let sessions = history_store.list_sessions()?;
    let audit = history_store.permission_audit_summary()?;

    let mut engine_counts = std::collections::BTreeMap::new();
    let mut archived_count = 0usize;
    for session in &sessions {
        let engine = match session.engine {
            EngineId::ClaudeCode => "claude-code",
            EngineId::Codex => "codex",
        };
        *engine_counts.entry(engine).or_insert(0usize) += 1;
        if session.archived {
            archived_count += 1;
        }
    }

    Ok(serde_json::json!({
        "schemaVersion": 1,
        "generatedAt": generated_at,
        "redactionNote": "诊断包只含本地事实与聚合统计；密钥、providers 配置、环境变量、审计明细不收集，自由文本已脱敏。",
        "app": {
            "name": "Helm",
            "version": platform.app_version,
            "tauriVersion": platform.tauri_version,
            "webviewVersion": platform.webview_version,
        },
        "platform": {
            "osName": platform.os_name,
            "osVersion": platform.os_version,
            "arch": platform.arch,
        },
        "settings": serde_json::to_value(&settings).map_err(|e| format!("序列化设置失败：{e}"))?,
        "sessionsSummary": {
            "total": sessions.len(),
            "archived": archived_count,
            "byEngine": engine_counts,
        },
        "permissionAuditSummary": serde_json::to_value(&audit)
            .map_err(|e| format!("序列化权限审计摘要失败：{e}"))?,
    }))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExport {
    pub path: String,
    pub bytes: usize,
}

#[tauri::command]
pub fn export_diagnostics_bundle(
    app: tauri::AppHandle,
    history_store: State<'_, SessionHistoryStore>,
) -> Result<Option<DiagnosticsExport>, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let platform = get_platform_info();
    let value = build_diagnostics_value(&history_store, &platform, &iso_now())?;
    let serialized =
        serde_json::to_string_pretty(&value).map_err(|e| format!("序列化诊断包失败：{e}"))?;
    let redacted = redact_text(&serialized);

    let dir = log_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败：{e}"))?;
    let default_name = format!("helm-diagnostics-{}.json", chrono_file_stamp());

    let Some(path) = app
        .dialog()
        .file()
        .set_title("导出诊断包")
        .set_file_name(&default_name)
        .add_filter("JSON", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = match path {
        FilePath::Path(path) => path,
        FilePath::Url(url) => return Err(format!("不支持的导出路径：{url}")),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建导出目录失败：{e}"))?;
    }
    std::fs::write(&path, redacted.as_bytes()).map_err(|e| format!("写入诊断包失败：{e}"))?;
    let bytes = redacted.as_bytes().len();

    // 导出记录写入日志目录（尽力而为；失败不推翻已成功的导出）。
    let marker = serde_json::json!({
        "path": path.to_string_lossy(),
        "exportedAt": iso_now(),
    });
    if let Err(err) = std::fs::write(dir.join("last-diagnostics-export.json"), marker.to_string()) {
        eprintln!("[helm] 写入诊断导出记录失败：{err}");
    }

    Ok(Some(DiagnosticsExport {
        path: path.to_string_lossy().to_string(),
        bytes,
    }))
}

// ─── 历史对话导入 ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportableHistoryEntry {
    pub engine: String,
    pub path: String,
    pub file_name: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub message_count: usize,
    pub first_message_preview: Option<String>,
    pub model: Option<String>,
    pub size_bytes: u64,
    pub modified_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportableHistoryScan {
    pub entries: Vec<ImportableHistoryEntry>,
    pub total_found: usize,
    pub skipped_too_large: usize,
    pub skipped_unparsable: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryImportResult {
    pub session_id: String,
    pub title: String,
    pub engine: String,
    pub cwd: String,
    pub imported_messages: usize,
    pub skipped_lines: usize,
}

/// 解析后的一个历史对话文件。
#[derive(Debug, Clone, Default)]
struct ParsedHistoryFile {
    messages: Vec<ImportedHistoryMessage>,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    engine: Option<EngineId>,
    skipped_lines: usize,
}

fn history_root_dir(engine: EngineId) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法定位用户主目录".to_string())?;
    match engine {
        EngineId::ClaudeCode => {
            let root = std::env::var("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".claude"));
            Ok(root.join("projects"))
        }
        EngineId::Codex => {
            let root = std::env::var("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".codex"));
            Ok(root.join("sessions"))
        }
    }
}

#[tauri::command]
pub fn list_importable_histories(engine: String) -> Result<ImportableHistoryScan, String> {
    let engine = parse_engine(&engine)?.ok_or_else(|| "扫描来源必须明确指定引擎".to_string())?;
    let root = history_root_dir(engine)?;
    if !root.is_dir() {
        return Ok(ImportableHistoryScan {
            entries: Vec::new(),
            total_found: 0,
            skipped_too_large: 0,
            skipped_unparsable: 0,
        });
    }

    let mut files: Vec<PathBuf> = Vec::new();
    collect_jsonl_files(&root, &mut files)?;
    let total_found = files.len();
    let mut skipped_too_large = 0usize;
    let mut skipped_unparsable = 0usize;

    let mut entries: Vec<ImportableHistoryEntry> = Vec::new();
    for path in files {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                skipped_unparsable += 1;
                continue;
            }
        };
        if meta.len() > MAX_HISTORY_FILE_BYTES {
            skipped_too_large += 1;
            continue;
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => {
                skipped_unparsable += 1;
                continue;
            }
        };
        let parsed = parse_history_contents(Some(engine), &contents);
        if parsed.messages.is_empty() {
            skipped_unparsable += 1;
            continue;
        }
        let modified_at_ms = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        entries.push(ImportableHistoryEntry {
            engine: engine_to_label(engine).to_string(),
            path: path.to_string_lossy().to_string(),
            file_name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            session_id: parsed.session_id,
            cwd: parsed.cwd,
            message_count: parsed.messages.len(),
            first_message_preview: parsed
                .messages
                .iter()
                .find(|message| message.role == "user")
                .map(|message| truncate_chars(&message.text, PREVIEW_MAX_CHARS)),
            model: parsed.model,
            size_bytes: meta.len(),
            modified_at_ms,
        });
    }

    // 最近修改优先；超出上限的条目丢弃但通过 total_found 表达，不静默。
    entries.sort_by(|a, b| b.modified_at_ms.cmp(&a.modified_at_ms));
    entries.truncate(MAX_SCAN_ENTRIES);
    Ok(ImportableHistoryScan {
        entries,
        total_found,
        skipped_too_large,
        skipped_unparsable,
    })
}

#[tauri::command]
pub fn import_history(
    history_store: State<'_, SessionHistoryStore>,
    source_path: String,
    engine: String,
    title_override: Option<String>,
) -> Result<HistoryImportResult, String> {
    let requested_engine = parse_engine(&engine)?;
    let path = PathBuf::from(&source_path);
    if !path.is_file() {
        return Err(format!("历史记录文件不存在：{source_path}"));
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("读取文件信息失败：{e}"))?;
    if meta.len() > MAX_HISTORY_FILE_BYTES {
        return Err(format!(
            "历史记录文件过大（{} 字节，上限 {MAX_HISTORY_FILE_BYTES} 字节）；请拆分后再导入",
            meta.len()
        ));
    }
    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("读取历史记录文件失败：{e}"))?;

    let parsed = parse_history_contents(requested_engine, &contents);
    if parsed.messages.is_empty() {
        return Err(
            "未解析到可导入的 user/assistant 消息；请确认这是 Claude Code / Codex 的 JSONL 记录"
                .to_string(),
        );
    }
    if parsed.messages.len() > MAX_IMPORT_MESSAGES {
        return Err(format!(
            "该对话包含 {} 条消息，超过单次导入上限 {MAX_IMPORT_MESSAGES}；请分批导入",
            parsed.messages.len()
        ));
    }
    let import_engine = parsed
        .engine
        .ok_or_else(|| "内部错误：解析结果缺少引擎标识".to_string())?;

    // cwd 解析顺序：记录内 cwd（必须真实存在）→ 设置默认目录（存在时）→ 用户主目录。
    let cwd = resolve_import_cwd(parsed.cwd.as_deref(), &history_store)?;

    let title = match title_override {
        Some(title) if !title.trim().is_empty() => title.trim().to_string(),
        _ => derive_history_title(&parsed.messages),
    };
    let session_id = history_store.import_history_session(
        NewSessionRecord {
            id: format!("{}-{:016x}", now_millis(), rand::random::<u64>()),
            engine: import_engine,
            model: parsed.model.clone().unwrap_or_default(),
            cwd: cwd.clone(),
            created_at: now_millis(),
        },
        &title,
        &parsed.messages,
    )?;
    Ok(HistoryImportResult {
        session_id,
        title,
        engine: engine_to_label(import_engine).to_string(),
        cwd,
        imported_messages: parsed.messages.len(),
        skipped_lines: parsed.skipped_lines,
    })
}

fn parse_engine(engine: &str) -> Result<Option<EngineId>, String> {
    match engine {
        "claude-code" => Ok(Some(EngineId::ClaudeCode)),
        "codex" => Ok(Some(EngineId::Codex)),
        "auto" => Ok(None),
        other => Err(format!("不支持的引擎标识：{other}")),
    }
}

fn engine_to_label(engine: EngineId) -> &'static str {
    match engine {
        EngineId::ClaudeCode => "claude-code",
        EngineId::Codex => "codex",
    }
}

fn resolve_import_cwd(
    recorded_cwd: Option<&str>,
    history_store: &SessionHistoryStore,
) -> Result<String, String> {
    if let Some(cwd) = recorded_cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        if Path::new(cwd).is_dir() {
            return Ok(cwd.to_string());
        }
    }
    if let Ok(settings) = crate::settings::load_app_settings_from_store(history_store) {
        let default = settings.general.default_directory.trim();
        if !default.is_empty() && Path::new(default).is_dir() {
            return Ok(default.to_string());
        }
    }
    dirs::home_dir()
        .map(|home| home.to_string_lossy().to_string())
        .ok_or_else(|| {
            "无法定位导入会话的工作目录：记录内目录、默认目录与主目录都不可用".to_string()
        })
}

fn derive_history_title(messages: &[ImportedHistoryMessage]) -> String {
    let first_user = messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.text.as_str())
        .unwrap_or("");
    let mut title = first_user
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    if title.is_empty() {
        title = "导入的对话".to_string();
    }
    let truncated = truncate_chars(&title, 42);
    format!("{truncated}（导入）")
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut result: String = text.chars().take(max).collect();
    result.push('…');
    result
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取目录失败：{}：{e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败：{e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// 解析一个 JSONL 历史记录文件内容。engine 为 None 时按内容形状自动探测。
fn parse_history_contents(engine: Option<EngineId>, contents: &str) -> ParsedHistoryFile {
    match engine {
        Some(EngineId::Codex) => parse_codex_rollout(contents),
        Some(EngineId::ClaudeCode) => parse_claude_jsonl(contents),
        None => {
            let codex = parse_codex_rollout(contents);
            if !codex.messages.is_empty() || codex.session_id.is_some() {
                codex
            } else {
                parse_claude_jsonl(contents)
            }
        }
    }
}

/// Claude Code 记录：每行一个对象，user/assistant 行带 message.content。
fn parse_claude_jsonl(contents: &str) -> ParsedHistoryFile {
    let mut parsed = ParsedHistoryFile {
        engine: Some(EngineId::ClaudeCode),
        ..Default::default()
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            parsed.skipped_lines += 1;
            continue;
        };
        let line_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(session_id) = value.get("sessionId").and_then(|v| v.as_str()) {
            parsed.session_id.get_or_insert(session_id.to_string());
        }
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            parsed.cwd.get_or_insert(cwd.to_string());
        }
        if !matches!(line_type, "user" | "assistant") {
            parsed.skipped_lines += 1;
            continue;
        }
        let Some(message) = value.get("message") else {
            parsed.skipped_lines += 1;
            continue;
        };
        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(line_type);
        if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
            parsed.model.get_or_insert(model.to_string());
        }
        let ts = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_rfc3339_millis)
            .unwrap_or(0);
        match claude_content_text(message.get("content")) {
            Some(text) if !text.trim().is_empty() => {
                match ImportedHistoryMessage::new(role, text, ts) {
                    Ok(message) => parsed.messages.push(message),
                    Err(_) => parsed.skipped_lines += 1,
                }
            }
            _ => parsed.skipped_lines += 1,
        }
    }
    parsed
}

/// Claude content 可能是纯字符串，也可能是 [{type:"text",text:...}] 数组。
fn claude_content_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
            }
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        _ => None,
    }
}

/// Codex rollout：session_meta 行带元数据，response_item 行带消息。
fn parse_codex_rollout(contents: &str) -> ParsedHistoryFile {
    let mut parsed = ParsedHistoryFile {
        engine: Some(EngineId::Codex),
        ..Default::default()
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            parsed.skipped_lines += 1;
            continue;
        };
        let line_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_rfc3339_millis)
            .unwrap_or(0);
        match line_type {
            "session_meta" => {
                if let Some(payload) = value.get("payload") {
                    if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                        parsed.session_id.get_or_insert(id.to_string());
                    }
                    if let Some(cwd) = payload.get("cwd").and_then(|v| v.as_str()) {
                        parsed.cwd.get_or_insert(cwd.to_string());
                    }
                }
            }
            "response_item" => {
                let Some(payload) = value.get("payload") else {
                    parsed.skipped_lines += 1;
                    continue;
                };
                if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
                    parsed.skipped_lines += 1;
                    continue;
                }
                let role = payload
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if let Some(model) = payload.get("model").and_then(|v| v.as_str()) {
                    parsed.model.get_or_insert(model.to_string());
                }
                match codex_content_text(payload.get("content")) {
                    Some(text) if !text.trim().is_empty() => {
                        match ImportedHistoryMessage::new(role, text, ts) {
                            Ok(message) => parsed.messages.push(message),
                            Err(_) => parsed.skipped_lines += 1,
                        }
                    }
                    _ => parsed.skipped_lines += 1,
                }
            }
            _ => parsed.skipped_lines += 1,
        }
    }
    parsed
}

/// Codex content 是 [{type:"input_text"|"output_text",text:...}] 数组。
fn codex_content_text(content: Option<&serde_json::Value>) -> Option<String> {
    let items = content?.as_array()?;
    let mut parts = Vec::new();
    for item in items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(item_type, "input_text" | "output_text" | "text") {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                parts.push(text.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// 解析 RFC3339 UTC 时间戳（如 2026-08-12T09:30:00.123Z）为毫秒；失败返回 None。
fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut millis: i64 = 0;
    let rest = &value[19..];
    let rest = if let Some(frac) = rest.strip_prefix('.') {
        let end = frac
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(frac.len());
        let digits = &frac[..end];
        if digits.is_empty() {
            return None;
        }
        let padded = format!("{digits:0<3}");
        millis = padded.get(..3)?.parse().ok()?;
        &frac[end..]
    } else {
        rest
    };
    // 只接受 UTC（Z / +00:00）；其他时区拒绝，宁可不带时间也不猜偏移。
    if !(rest.is_empty() || rest.eq_ignore_ascii_case("z") || rest == "+00:00") {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_sample() -> String {
        let user = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "帮我检查登录流程"},
            "timestamp": "2026-08-12T09:30:00.123Z",
            "sessionId": "abc-1",
            "cwd": "D:/Projects/helm"
        });
        let assistant = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "model": "claude-sonnet-4",
                "content": [{"type": "text", "text": "好的，我先看代码。"}]
            },
            "timestamp": "2026-08-12T09:30:05Z"
        });
        format!("{user}\n{assistant}\n{{\"type\":\"summary\"}}\nnot-json-line\n")
    }

    fn codex_sample() -> String {
        let meta = serde_json::json!({
            "timestamp": "2026-08-12T10:00:00Z",
            "type": "session_meta",
            "payload": {"id": "roll-1", "cwd": "D:/Projects/demo"}
        });
        let user = serde_json::json!({
            "timestamp": "2026-08-12T10:00:01.500Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "运行测试"}]
            }
        });
        let assistant = serde_json::json!({
            "timestamp": "2026-08-12T10:00:09Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "model": "gpt-5-codex",
                "content": [{"type": "output_text", "text": "全部通过。"}]
            }
        });
        let noise = serde_json::json!({
            "timestamp": "2026-08-12T10:00:10Z",
            "type": "event_msg",
            "payload": {"type": "agent_reasoning"}
        });
        format!("{meta}\n{user}\n{assistant}\n{noise}\n")
    }

    #[test]
    fn parses_claude_jsonl_messages_meta_and_skips() {
        let parsed = parse_claude_jsonl(&claude_sample());
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.messages[0].role, "user");
        assert_eq!(parsed.messages[0].text, "帮我检查登录流程");
        assert_eq!(parsed.messages[0].ts_millis, 1_786_527_000_123);
        assert_eq!(parsed.messages[1].role, "assistant");
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(parsed.cwd.as_deref(), Some("D:/Projects/helm"));
        assert_eq!(parsed.session_id.as_deref(), Some("abc-1"));
        assert_eq!(parsed.skipped_lines, 2);
    }

    #[test]
    fn parses_codex_rollout_messages_and_meta() {
        let parsed = parse_codex_rollout(&codex_sample());
        assert_eq!(parsed.messages.len(), 2);
        assert_eq!(parsed.session_id.as_deref(), Some("roll-1"));
        assert_eq!(parsed.model.as_deref(), Some("gpt-5-codex"));
        // 无小数毫秒的时间戳按整秒换算：2026-08-12T10:00:01.500Z 之外那两条
        assert!(parsed.messages[1].ts_millis > parsed.messages[0].ts_millis);
        assert_eq!(parsed.skipped_lines, 1);
    }

    #[test]
    fn auto_detection_prefers_codex_shape_then_claude() {
        let codex = parse_history_contents(None, &codex_sample());
        assert_eq!(codex.engine, Some(EngineId::Codex));
        let claude = parse_history_contents(None, &claude_sample());
        assert_eq!(claude.engine, Some(EngineId::ClaudeCode));
    }

    #[test]
    fn imported_message_rejects_unexpected_roles() {
        assert!(ImportedHistoryMessage::new("system", "x", 0).is_err());
        assert!(ImportedHistoryMessage::new("tool", "x", 0).is_err());
        assert!(ImportedHistoryMessage::new("user", "x", 0).is_ok());
    }

    #[test]
    fn rfc3339_parses_utc_and_rejects_other_timezones() {
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_millis("2026-08-12T09:30:00.123Z"),
            Some(1_786_527_000_123)
        );
        assert_eq!(parse_rfc3339_millis("2026-08-12T09:30:00+08:00"), None);
        assert_eq!(parse_rfc3339_millis("not-a-time"), None);
    }

    #[test]
    fn title_derives_from_first_user_line_with_import_suffix() {
        let messages = vec![
            ImportedHistoryMessage::new("user", "第一行标题\n第二行", 0).unwrap(),
            ImportedHistoryMessage::new("assistant", "回复", 1).unwrap(),
        ];
        assert_eq!(derive_history_title(&messages), "第一行标题（导入）");
    }

    #[test]
    fn truncate_keeps_char_budget() {
        assert_eq!(truncate_chars("一二三四五", 3), "一二三…");
        assert_eq!(truncate_chars("abc", 5), "abc");
    }

    #[test]
    fn civil_days_roundtrip() {
        let (year, month, day) = civil_from_days(days_from_civil(2026, 8, 12));
        assert_eq!((year, month, day), (2026, 8, 12));
    }

    #[test]
    fn iso_now_has_utc_suffix() {
        assert!(iso_now().ends_with('Z'));
        assert_eq!(iso_now().len(), 20);
    }
}
