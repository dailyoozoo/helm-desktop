//! 扩展管理：技能、MCP 服务器、子代理、斜杠命令、钩子。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 技能元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub enabled: bool,
    pub path: String,
    pub engine: String,
    pub trigger: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillScope {
    #[default]
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Builtin,
    Market,
    Custom,
    Plugin,
}

/// MCP 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub transport: McpTransport,
    pub enabled: bool,
    pub status: McpStatus,
    /// 最近一次测试连接的结果（持久化在 ~/.helm/mcp-status.json，跨重启保留）
    #[serde(default, rename = "lastTestedAt")]
    pub last_tested_at: Option<u64>,
    #[serde(default, rename = "toolCount")]
    pub tool_count: Option<u32>,
    #[serde(default, rename = "lastError")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Sse,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpStatus {
    Connected,
    Disconnected,
    Error,
}

/// MCP 工具信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
}

/// 子代理配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subagent {
    pub id: String,
    pub name: String,
    pub model: String,
    pub role: String,
    pub tools: String,
    pub auto: bool,
    pub prompt: String,
    #[serde(default)]
    pub scope: SkillScope,
}

/// 斜杠命令
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    pub id: String,
    pub trigger: String,
    pub description: String,
    pub scope: SkillScope,
    pub enabled: bool,
    pub body: String,
    pub engine: String,
    #[serde(default)]
    pub source: CommandSource,
    #[serde(default, rename = "argumentHint")]
    pub argument_hint: Option<String>,
}

/// 命令来源：扩展中心管理 / 引擎原生（用户级、项目级）/ 内置。
/// 同 trigger 冲突时优先级：extension > engine-project > engine-user > builtin（变更-03 A.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandSource {
    #[default]
    Extension,
    EngineUser,
    EngineProject,
    Builtin,
}

impl CommandSource {
    fn priority(self) -> u8 {
        match self {
            CommandSource::Extension => 0,
            CommandSource::EngineProject => 1,
            CommandSource::EngineUser => 2,
            CommandSource::Builtin => 3,
        }
    }
}

/// 钩子配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub id: String,
    pub event: HookEvent,
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub command: String,
    pub description: String,
    pub enabled: bool,
    #[serde(default)]
    pub scope: SkillScope,
}

/// Claude Code hook 事件全集（变更-05 从 3 种扩到 9 种）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Notification,
    Stop,
    SubagentStop,
    PreCompact,
    SessionStart,
    SessionEnd,
}

impl HookEvent {
    fn as_str(&self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::Notification => "Notification",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
        }
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| "无法获取用户目录".to_string())
}

fn claude_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".claude"))
}

fn claude_settings_path() -> Result<PathBuf, String> {
    Ok(claude_dir()?.join("settings.json"))
}

fn claude_mcp_config_path() -> Result<PathBuf, String> {
    if let Ok(config_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let config_dir = config_dir.trim();
        if !config_dir.is_empty() {
            return Ok(PathBuf::from(config_dir).join(".claude.json"));
        }
    }
    Ok(home_dir()?.join(".claude.json"))
}

fn codex_config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

fn codex_prompts_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex").join("prompts"))
}

fn codex_skills_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".codex").join("skills"))
}

fn claude_agents_dir() -> Result<PathBuf, String> {
    Ok(claude_dir()?.join("agents"))
}

fn claude_commands_dir() -> Result<PathBuf, String> {
    Ok(claude_dir()?.join("commands"))
}

/// 项目级 `.claude` 子目录（skills / agents / commands / settings.json）
fn project_claude_dir(project_dir: &str) -> Result<PathBuf, String> {
    let trimmed = project_dir.trim();
    if trimmed.is_empty() {
        return Err("项目目录为空".to_string());
    }
    Ok(PathBuf::from(trimmed).join(".claude"))
}
/// Claude Code marketplace plugin 安装目录
fn claude_plugins_marketplaces_dir() -> Result<PathBuf, String> {
    Ok(claude_dir()?.join("plugins").join("marketplaces"))
}

/// 扫描 Claude Code marketplace plugin 里的 skills。
///
/// 目录结构：~/.claude/plugins/marketplaces/<plugin>/skills/<skill>/SKILL.md
/// 产出 ID 格式：plugin:<plugin>:<skill>（如 plugin:caveman:caveman）
/// Trigger 格式：/<plugin>:<skill>（如 /caveman:caveman）
/// Plugin skill 在 Helm 中只读展示，不支持通过 .helm-disabled 启停。
pub fn list_plugin_skills_from_marketplaces() -> Result<Vec<Skill>, String> {
    list_plugin_skills_from_dir(&claude_plugins_marketplaces_dir()?)
}

/// 从指定目录扫描 marketplace plugin skills（测试可注入目录）。
pub fn list_plugin_skills_from_dir(marketplaces_dir: &Path) -> Result<Vec<Skill>, String> {
    if !marketplaces_dir.exists() {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    let entries = std::fs::read_dir(marketplaces_dir)
        .map_err(|e| format!("读取 Claude 插件市场目录失败: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取插件目录项失败: {}", e))?;
        if !entry.path().is_dir() {
            continue;
        }
        let plugin_name = entry.file_name().to_string_lossy().to_string();
        if plugin_name.starts_with('.') {
            continue;
        }
        let plugin_path = entry.path();
        scan_skills_in_plugin_dir(&plugin_path, &plugin_name, &mut skills)?;
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(skills)
}

/// 递归扫描插件目录下的 skills/ 子目录，发现 SKILL.md 文件。
fn scan_skills_in_plugin_dir(
    dir: &Path,
    plugin_name: &str,
    skills: &mut Vec<Skill>,
) -> Result<(), String> {
    let skills_dir = dir.join("skills");
    if skills_dir.exists() {
        read_skills_from_dir_with_prefix(&skills_dir, plugin_name, skills)?;
    }
    // 某些插件（如 claude-plugins-official）有嵌套的 plugins/<name>/skills/
    let plugins_dir = dir.join("plugins");
    if plugins_dir.exists() {
        let entries = std::fs::read_dir(&plugins_dir)
            .map_err(|e| format!("读取嵌套插件目录失败: {}", e))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("读取嵌套插件目录项失败: {}", e))?;
            if entry.path().is_dir() {
                scan_skills_in_plugin_dir(&entry.path(), plugin_name, skills)?;
            }
        }
    }
    Ok(())
}

/// 从目录读取 skill，用 plugin_name 作为命名空间前缀。
fn read_skills_from_dir_with_prefix(
    dir: &Path,
    plugin_name: &str,
    skills: &mut Vec<Skill>,
) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取技能目录失败: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_id = entry.file_name().to_string_lossy().to_string();
        if skill_id.is_empty() || skill_id.starts_with('.') {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        let readme_md = path.join("README.md");
        let meta_file = if skill_md.exists() {
            skill_md
        } else if readme_md.exists() {
            readme_md
        } else {
            continue;
        };
        let content = std::fs::read_to_string(&meta_file).unwrap_or_default();
        let name = extract_title(&content).unwrap_or_else(|| skill_id.clone());
        let description = extract_description(&content);
        let namespaced_id = format!("{plugin_name}:{skill_id}");
        let trigger = format!("/{namespaced_id}");
        skills.push(Skill {
            trigger,
            id: format!("plugin:{namespaced_id}"),
            name,
            description,
            scope: SkillScope::Global,
            source: SkillSource::Plugin,
            enabled: true,
            path: path.to_string_lossy().to_string(),
            engine: "claude-code".to_string(),
        });
    }
    Ok(())
}

/// MCP 连接状态持久化文件（Helm 自有，不进引擎配置）
fn mcp_status_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".helm").join("mcp-status.json"))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn shared_config_write_guard() -> Result<MutexGuard<'static, ()>, String> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "CLI 共享配置写锁中毒".to_string())
}

fn write_shared_config_atomically(path: &Path, content: &[u8]) -> Result<(), String> {
    write_shared_config_atomically_with(path, content, |_| Ok(()))
}

fn write_shared_config_atomically_with<F>(
    path: &Path,
    content: &[u8],
    before_replace: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let parent = path
        .parent()
        .ok_or_else(|| "CLI 共享配置缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let temporary = parent.join(format!(
        ".helm-config-{}-{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("创建配置临时文件失败: {e}"))?;
        file.write_all(content)
            .map_err(|e| format!("写入配置临时文件失败: {e}"))?;
        file.flush()
            .map_err(|e| format!("刷新配置临时文件失败: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("同步配置临时文件失败: {e}"))?;
        before_replace(&temporary)?;
        replace_shared_config(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_shared_config(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(format!(
            "原子替换 CLI 共享配置失败: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_shared_config(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|e| format!("原子替换 CLI 共享配置失败: {e}"))?;
    if let Some(parent) = destination.parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| format!("同步 CLI 共享配置目录失败: {e}"))?;
    }
    Ok(())
}

fn read_settings(path: &Path) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取 settings.json 失败: {}", e))?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 settings.json 失败: {}", e))?;
    if !settings.is_object() {
        settings = serde_json::json!({});
    }
    Ok(settings)
}

fn write_settings(path: &Path, settings: &serde_json::Value) -> Result<(), String> {
    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("序列化 settings.json 失败: {}", e))?;
    write_shared_config_atomically(path, content.as_bytes())
        .map_err(|e| format!("写入 settings.json 失败: {e}"))
}

fn update_settings<F>(path: &Path, update: F) -> Result<(), String>
where
    F: FnOnce(&mut serde_json::Value) -> Result<(), String>,
{
    let _guard = shared_config_write_guard()?;
    let mut settings = read_settings(path)?;
    update(&mut settings)?;
    write_settings(path, &settings)
}

fn read_toml_config(path: &Path) -> Result<toml::Value, String> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取 config.toml 失败: {}", e))?;
    if content.trim().is_empty() {
        return Ok(toml::Value::Table(toml::map::Map::new()));
    }
    content
        .parse::<toml::Value>()
        .map_err(|e| format!("解析 config.toml 失败: {}", e))
}

fn write_toml_config(path: &Path, config: &toml::Value) -> Result<(), String> {
    let content =
        toml::to_string_pretty(config).map_err(|e| format!("序列化 config.toml 失败: {}", e))?;
    write_shared_config_atomically(path, content.as_bytes())
        .map_err(|e| format!("写入 config.toml 失败: {e}"))
}

fn update_toml_config<F>(path: &Path, update: F) -> Result<(), String>
where
    F: FnOnce(&mut toml::Value) -> Result<(), String>,
{
    let _guard = shared_config_write_guard()?;
    let mut config = read_toml_config(path)?;
    update(&mut config)?;
    write_toml_config(path, &config)
}

fn toml_table_mut<'a>(
    value: &'a mut toml::Value,
    key: &str,
) -> Result<&'a mut toml::map::Map<String, toml::Value>, String> {
    let root = value
        .as_table_mut()
        .ok_or_else(|| "Codex 配置顶层不是 TOML 表".to_string())?;
    if root.get(key).and_then(toml::Value::as_table).is_none() {
        root.insert(key.to_string(), toml::Value::Table(toml::map::Map::new()));
    }
    root.get_mut(key)
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| format!("配置字段 {key} 不是 TOML 表"))
}

fn object_mut<'a>(
    value: &'a mut serde_json::Value,
    key: &str,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, String> {
    if value.get(key).and_then(|v| v.as_object()).is_none() {
        value[key] = serde_json::json!({});
    }
    value
        .get_mut(key)
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| format!("配置字段 {key} 不是对象"))
}

fn array_mut<'a>(
    value: &'a mut serde_json::Value,
    key: &str,
) -> Result<&'a mut Vec<serde_json::Value>, String> {
    if value.get(key).and_then(|v| v.as_array()).is_none() {
        value[key] = serde_json::json!([]);
    }
    value
        .get_mut(key)
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| format!("配置字段 {key} 不是数组"))
}

fn safe_file_stem(input: &str) -> Result<String, String> {
    let trimmed = input.trim().trim_start_matches('/');
    let mut output = String::new();
    let mut previous_dash = false;
    for ch in trimmed.chars() {
        let next = if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            Some(ch.to_ascii_lowercase())
        } else if ch.is_whitespace() || ch == '/' || ch == '\\' || ch == '.' {
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            if ch == '-' {
                if previous_dash {
                    continue;
                }
                previous_dash = true;
            } else {
                previous_dash = false;
            }
            output.push(ch);
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        return Err("名称必须包含可用于文件名的英文、数字、短横线或下划线".to_string());
    }
    Ok(output)
}

fn normalize_trigger(trigger: &str) -> String {
    let trimmed = trigger.trim();
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn normalize_engine(engine: &str) -> String {
    let trimmed = engine.trim().to_lowercase();
    match trimmed.as_str() {
        "claude-code" | "claude_code" | "claudecode" => "claude-code".to_string(),
        "codex" => "codex".to_string(),
        _ => "all".to_string(),
    }
}

fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let mut meta = HashMap::new();
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (meta, content.to_string());
    }

    let mut body_lines = Vec::new();
    let mut in_meta = true;
    for line in lines {
        if in_meta && line.trim() == "---" {
            in_meta = false;
            continue;
        }
        if in_meta {
            if let Some((key, value)) = line.split_once(':') {
                meta.insert(
                    key.trim().to_string(),
                    value.trim().trim_matches('"').to_string(),
                );
            }
        } else {
            body_lines.push(line);
        }
    }
    (meta, body_lines.join("\n").trim().to_string())
}

fn markdown_with_frontmatter(meta: &[(&str, String)], body: &str) -> String {
    let mut content = String::from("---\n");
    for (key, value) in meta {
        content.push_str(key);
        content.push_str(": ");
        content.push_str(value.replace('\n', " ").trim());
        content.push('\n');
    }
    content.push_str("---\n\n");
    content.push_str(body.trim());
    content.push('\n');
    content
}

/// 扫描 Claude Code 技能目录（含 .helm-disabled 停用区）。
/// 旧版把停用状态写进 settings.json 的 skillsDisabled 键——2026-07 实测 claude CLI
/// 不认该键（skill 照常加载，变更-03 A.3），因此停用改为目录移动，旧键一次性迁移。
pub fn list_skills(
    engine: Option<String>,
    project_dir: Option<String>,
) -> Result<Vec<Skill>, String> {
    if engine.as_deref() == Some("codex") {
        let mut skills =
            list_skills_from_dir_for_engine(&codex_skills_dir()?, "codex", "$", false)?;
        if let Some(project_dir) = project_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let project_dir = PathBuf::from(project_dir).join(".codex").join("skills");
            for mut skill in list_skills_from_dir_for_engine(&project_dir, "codex", "$", false)? {
                skill.scope = SkillScope::Project;
                skill.id = format!("proj:{}", skill.id);
                skills.push(skill);
            }
        }
        skills.sort_by(|a, b| a.id.cmp(&b.id));
        return Ok(skills);
    }
    let skills_dir = claude_dir()?.join("skills");
    migrate_legacy_disabled_skills(&skills_dir, &claude_settings_path()?)?;
    let mut skills = list_skills_from_dir(&skills_dir)?;
    if let Some(project_dir) = project_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let project_skills_dir = project_claude_dir(project_dir)?.join("skills");
        for mut skill in list_skills_from_dir(&project_skills_dir)? {
            skill.scope = SkillScope::Project;
            skill.id = format!("proj:{}", skill.id);
            skills.push(skill);
        }
    }
    // 合并 Claude Code marketplace plugin 里的 skills
    match list_plugin_skills_from_marketplaces() {
        Ok(plugin_skills) => skills.extend(plugin_skills),
        Err(e) => eprintln!("读取插件市场技能失败（忽略）: {e}"),
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(skills)
}

pub fn list_skills_from_dir(skills_dir: &Path) -> Result<Vec<Skill>, String> {
    list_skills_from_dir_for_engine(skills_dir, "claude-code", "/", true)
}

fn list_skills_from_dir_for_engine(
    skills_dir: &Path,
    engine: &str,
    prefix: &str,
    include_disabled: bool,
) -> Result<Vec<Skill>, String> {
    let mut skills = Vec::new();
    read_skills_from_dir(skills_dir, true, engine, prefix, &mut skills)?;
    if include_disabled {
        read_skills_from_dir(
            &skills_dir.join(".helm-disabled"),
            false,
            engine,
            prefix,
            &mut skills,
        )?;
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(skills)
}

fn read_skills_from_dir(
    dir: &Path,
    enabled: bool,
    engine: &str,
    prefix: &str,
    skills: &mut Vec<Skill>,
) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| format!("读取技能目录失败: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let skill_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if skill_id.is_empty() || skill_id == ".helm-disabled" {
            continue;
        }

        // 读取 SKILL.md 或 README.md 获取元信息
        let skill_md = path.join("SKILL.md");
        let readme_md = path.join("README.md");

        let meta_file = if skill_md.exists() {
            skill_md
        } else if readme_md.exists() {
            readme_md
        } else {
            continue;
        };

        let content = std::fs::read_to_string(&meta_file).unwrap_or_default();

        // 简单解析：第一个 # 标题作为名称，第一段作为描述
        let name = extract_title(&content).unwrap_or_else(|| skill_id.clone());
        let description = extract_description(&content);
        // 市场安装的技能带 .helm-market.json 标记（变更-05）
        let source = if path.join(".helm-market.json").exists() {
            SkillSource::Market
        } else {
            SkillSource::Custom
        };

        skills.push(Skill {
            trigger: format!("{prefix}{skill_id}"),
            id: skill_id,
            name,
            description,
            scope: SkillScope::Global,
            source,
            enabled,
            path: path.to_string_lossy().to_string(),
            engine: engine.to_string(),
        });
    }

    Ok(())
}

/// 把 settings.json 旧 skillsDisabled 键里的技能一次性迁移为目录移动，然后删除该键。
fn migrate_legacy_disabled_skills(skills_dir: &Path, settings_path: &Path) -> Result<(), String> {
    if !settings_path.exists() {
        return Ok(());
    }
    let _guard = shared_config_write_guard()?;
    let mut settings = read_settings(settings_path)?;
    let Some(ids) = settings.get("skillsDisabled").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    let ids: Vec<String> = ids
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for id in &ids {
        let enabled_path = skills_dir.join(id);
        if !enabled_path.is_dir() {
            continue;
        }
        let disabled_dir = skills_dir.join(".helm-disabled");
        let disabled_path = disabled_dir.join(id);
        if disabled_path.exists() {
            continue;
        }
        std::fs::create_dir_all(&disabled_dir)
            .map_err(|e| format!("创建技能停用目录失败: {}", e))?;
        std::fs::rename(&enabled_path, &disabled_path)
            .map_err(|e| format!("迁移停用技能失败: {}", e))?;
    }
    if let Some(object) = settings.as_object_mut() {
        object.remove("skillsDisabled");
    }
    write_settings(settings_path, &settings)
}

fn extract_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            return Some(trimmed[2..].trim().to_string());
        }
    }
    None
}

fn extract_description(content: &str) -> String {
    let mut in_desc = false;
    let mut desc_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("# ") {
            in_desc = true;
            continue;
        }

        if in_desc {
            if trimmed.is_empty() {
                if !desc_lines.is_empty() {
                    break;
                }
                continue;
            }

            if trimmed.starts_with("#") {
                break;
            }

            desc_lines.push(trimmed);
        }
    }

    desc_lines.join(" ").chars().take(200).collect()
}

/// 切换技能启用状态：目录在 skills/ 与 skills/.helm-disabled/ 之间移动
/// （settings.json 的 skillsDisabled 键对 claude CLI 无效，不再写入）。
pub fn toggle_skill(
    skill_id: &str,
    enabled: bool,
    project_dir: Option<String>,
) -> Result<(), String> {
    if let Some(id) = skill_id.strip_prefix("proj:") {
        let project_dir = project_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "项目级技能需要提供项目目录".to_string())?;
        let skills_dir = project_claude_dir(project_dir)?.join("skills");
        return toggle_skill_in_dir(&skills_dir, id, enabled);
    }
    let skills_dir = claude_dir()?.join("skills");
    migrate_legacy_disabled_skills(&skills_dir, &claude_settings_path()?)?;
    toggle_skill_in_dir(&skills_dir, skill_id, enabled)
}

pub fn toggle_skill_in_dir(skills_dir: &Path, skill_id: &str, enabled: bool) -> Result<(), String> {
    let id = safe_file_stem(skill_id)?;
    let enabled_path = skills_dir.join(&id);
    let disabled_dir = skills_dir.join(".helm-disabled");
    let disabled_path = disabled_dir.join(&id);
    if enabled {
        if !disabled_path.is_dir() {
            return Ok(());
        }
        if enabled_path.exists() {
            return Err(format!("启用目录中已存在同名技能：{id}"));
        }
        std::fs::rename(&disabled_path, &enabled_path).map_err(|e| format!("启用技能失败: {}", e))
    } else {
        if !enabled_path.is_dir() {
            return Ok(());
        }
        if disabled_path.exists() {
            return Err(format!("停用目录中已存在同名技能：{id}"));
        }
        std::fs::create_dir_all(&disabled_dir)
            .map_err(|e| format!("创建技能停用目录失败: {}", e))?;
        std::fs::rename(&enabled_path, &disabled_path).map_err(|e| format!("停用技能失败: {}", e))
    }
}

/// 列出 MCP 服务器配置（合并 ~/.helm/mcp-status.json 里的最近连接状态）
pub fn list_mcp_servers() -> Result<Vec<McpServer>, String> {
    migrate_legacy_claude_mcp_config()?;
    let mut servers = list_mcp_servers_from_settings_path(&claude_mcp_config_path()?)?;
    for server in list_mcp_servers_from_codex_config_path(&codex_config_path()?)? {
        if !servers.iter().any(|existing| existing.name == server.name) {
            servers.push(server);
        }
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    if let Ok(status_map) = read_mcp_status(&mcp_status_path()?) {
        for server in servers.iter_mut() {
            if let Some(entry) = status_map.get(&server.name) {
                server.last_tested_at = Some(entry.tested_at);
                server.tool_count = entry.tool_count;
                server.last_error = entry.error.clone();
                server.status = if entry.ok {
                    McpStatus::Connected
                } else {
                    McpStatus::Error
                };
            }
        }
    }
    Ok(servers)
}

/// MCP 最近一次连接测试结果（Helm 自有持久化，不写引擎配置）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStatusEntry {
    pub ok: bool,
    #[serde(rename = "toolCount")]
    pub tool_count: Option<u32>,
    pub error: Option<String>,
    #[serde(rename = "testedAt")]
    pub tested_at: u64,
}

pub fn read_mcp_status(path: &Path) -> Result<HashMap<String, McpStatusEntry>, String> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取 MCP 状态文件失败: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("解析 MCP 状态文件失败: {}", e))
}

pub fn record_mcp_status_to_path(
    path: &Path,
    server_name: &str,
    result: &Result<Vec<McpTool>, String>,
) -> Result<(), String> {
    let _guard = shared_config_write_guard()?;
    // 状态文件损坏时按空表重建，不阻塞测试连接主流程
    let mut status_map = read_mcp_status(path).unwrap_or_default();
    let entry = match result {
        Ok(tools) => McpStatusEntry {
            ok: true,
            tool_count: Some(tools.len() as u32),
            error: None,
            tested_at: unix_now(),
        },
        Err(error) => McpStatusEntry {
            ok: false,
            tool_count: None,
            error: Some(error.clone()),
            tested_at: unix_now(),
        },
    };
    status_map.insert(server_name.to_string(), entry);
    let content = serde_json::to_string_pretty(&status_map)
        .map_err(|e| format!("序列化 MCP 状态失败: {}", e))?;
    write_shared_config_atomically(path, content.as_bytes())
        .map_err(|e| format!("写入 MCP 状态失败: {e}"))
}

pub fn record_mcp_status(server_name: &str, result: &Result<Vec<McpTool>, String>) {
    if let Ok(path) = mcp_status_path() {
        let _ = record_mcp_status_to_path(&path, server_name, result);
    }
}

pub fn forget_mcp_status(server_name: &str) {
    let Ok(path) = mcp_status_path() else {
        return;
    };
    let Ok(_guard) = shared_config_write_guard() else {
        return;
    };
    let Ok(mut status_map) = read_mcp_status(&path) else {
        return;
    };
    if status_map.remove(server_name).is_some() {
        if let Ok(content) = serde_json::to_string_pretty(&status_map) {
            let _ = write_shared_config_atomically(&path, content.as_bytes());
        }
    }
}

pub fn list_mcp_servers_from_settings_path(path: &Path) -> Result<Vec<McpServer>, String> {
    let settings = read_settings(path)?;

    let mut servers = Vec::new();

    if let Some(mcp_servers) = settings.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, config) in mcp_servers {
            let url = config
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let type_field = config.get("type").and_then(|v| v.as_str());
            let transport = match type_field {
                Some("http") => McpTransport::Http,
                Some("sse") => McpTransport::Sse,
                _ if !url.is_empty() && config.get("command").is_none() => McpTransport::Sse,
                _ => McpTransport::Stdio,
            };
            let is_remote = transport != McpTransport::Stdio;
            let command = config
                .get(if is_remote { "url" } else { "command" })
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let args = config
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let env = config
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            servers.push(McpServer {
                name: name.clone(),
                command,
                args,
                env,
                transport,
                enabled: true,
                status: McpStatus::Disconnected,
                last_tested_at: None,
                tool_count: None,
                last_error: None,
            });
        }
    }

    Ok(servers)
}

fn mcp_remote_url_error(transport: McpTransport) -> String {
    match transport {
        McpTransport::Stdio => "MCP stdio 服务器必须填写启动命令".to_string(),
        McpTransport::Sse => "MCP SSE 服务器必须填写 URL".to_string(),
        McpTransport::Http => "MCP HTTP 服务器必须填写 URL".to_string(),
    }
}

pub fn save_mcp_server(server: McpServer) -> Result<(), String> {
    migrate_legacy_claude_mcp_config()?;
    save_mcp_server_to_settings_path(&claude_mcp_config_path()?, server.clone())?;
    save_mcp_server_to_codex_config_path(&codex_config_path()?, server)
}

fn migrate_legacy_claude_mcp_config() -> Result<(), String> {
    let legacy_path = claude_settings_path()?;
    let target_path = claude_mcp_config_path()?;
    migrate_legacy_claude_mcp_config_at(&legacy_path, &target_path)
}

fn migrate_legacy_claude_mcp_config_at(
    legacy_path: &Path,
    target_path: &Path,
) -> Result<(), String> {
    if legacy_path == target_path || !legacy_path.exists() {
        return Ok(());
    }

    let _guard = shared_config_write_guard()?;
    let mut legacy = read_settings(&legacy_path)?;
    let Some(legacy_servers) = legacy
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .cloned()
    else {
        return Ok(());
    };
    if legacy_servers.is_empty() {
        return Ok(());
    }

    let mut target = read_settings(&target_path)?;
    let target_servers = object_mut(&mut target, "mcpServers")?;
    for (name, config) in legacy_servers {
        target_servers.entry(name).or_insert(config);
    }
    let target_content = serde_json::to_string_pretty(&target)
        .map_err(|e| format!("序列化 Claude MCP 配置失败: {e}"))?;
    write_shared_config_atomically(&target_path, target_content.as_bytes())
        .map_err(|e| format!("迁移 Claude MCP 配置失败: {e}"))?;

    if let Some(object) = legacy.as_object_mut() {
        object.remove("mcpServers");
    }
    let legacy_content = serde_json::to_string_pretty(&legacy)
        .map_err(|e| format!("序列化 Claude settings 失败: {e}"))?;
    write_shared_config_atomically(&legacy_path, legacy_content.as_bytes())
        .map_err(|e| format!("清理旧 Claude MCP 配置失败: {e}"))
}

pub fn save_mcp_server_to_settings_path(path: &Path, server: McpServer) -> Result<(), String> {
    if server.name.trim().is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    if server.command.trim().is_empty() {
        return Err(mcp_remote_url_error(server.transport));
    }

    let name = server.name.trim().to_string();
    let config = match server.transport {
        McpTransport::Stdio => serde_json::json!({
            "command": server.command.trim(),
            "args": server.args,
            "env": server.env
        }),
        McpTransport::Sse => serde_json::json!({
            "type": "sse",
            "url": server.command.trim()
        }),
        McpTransport::Http => serde_json::json!({
            "type": "http",
            "url": server.command.trim()
        }),
    };
    update_settings(path, move |settings| {
        object_mut(settings, "mcpServers")?.insert(name, config);
        Ok(())
    })
}

pub fn delete_mcp_server(name: &str) -> Result<(), String> {
    migrate_legacy_claude_mcp_config()?;
    delete_mcp_server_from_settings_path(&claude_mcp_config_path()?, name)?;
    delete_mcp_server_from_codex_config_path(&codex_config_path()?, name)
}

pub fn delete_mcp_server_from_settings_path(path: &Path, name: &str) -> Result<(), String> {
    update_settings(path, |settings| {
        if let Some(mcp_servers) = settings
            .get_mut("mcpServers")
            .and_then(|v| v.as_object_mut())
        {
            mcp_servers.remove(name);
        }
        Ok(())
    })
}

pub fn list_mcp_servers_from_codex_config_path(path: &Path) -> Result<Vec<McpServer>, String> {
    let config = read_toml_config(path)?;
    let mut servers = Vec::new();

    if let Some(mcp_servers) = config.get("mcp_servers").and_then(toml::Value::as_table) {
        for (name, server_config) in mcp_servers {
            let url = server_config
                .get("url")
                .and_then(toml::Value::as_str)
                .unwrap_or_default();
            let is_sse = !url.is_empty() && server_config.get("command").is_none();
            let command = server_config
                .get(if is_sse { "url" } else { "command" })
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = server_config
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|value| value.as_str().map(ToString::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let env = server_config
                .get("env")
                .and_then(toml::Value::as_table)
                .map(|table| {
                    table
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();

            servers.push(McpServer {
                name: name.clone(),
                command,
                args,
                env,
                transport: if is_sse {
                    McpTransport::Sse
                } else {
                    McpTransport::Stdio
                },
                enabled: true,
                status: McpStatus::Disconnected,
                last_tested_at: None,
                tool_count: None,
                last_error: None,
            });
        }
    }

    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(servers)
}

pub fn save_mcp_server_to_codex_config_path(path: &Path, server: McpServer) -> Result<(), String> {
    if server.name.trim().is_empty() {
        return Err("MCP 服务器名称不能为空".to_string());
    }
    if server.command.trim().is_empty() {
        return Err(mcp_remote_url_error(server.transport));
    }

    let name = server.name.trim().to_string();
    let mut server_table = toml::map::Map::new();
    match server.transport {
        McpTransport::Stdio => {
            server_table.insert(
                "command".to_string(),
                toml::Value::String(server.command.trim().to_string()),
            );
            server_table.insert(
                "args".to_string(),
                toml::Value::Array(server.args.into_iter().map(toml::Value::String).collect()),
            );
            if !server.env.is_empty() {
                let mut env_table = toml::map::Map::new();
                let mut entries = server.env.into_iter().collect::<Vec<_>>();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, value) in entries {
                    env_table.insert(key, toml::Value::String(value));
                }
                server_table.insert("env".to_string(), toml::Value::Table(env_table));
            }
        }
        // Codex config.toml 对远程 MCP 只认 url，SSE 与 streamable HTTP 由 CLI 自行协商
        McpTransport::Sse | McpTransport::Http => {
            server_table.insert(
                "url".to_string(),
                toml::Value::String(server.command.trim().to_string()),
            );
        }
    }
    update_toml_config(path, move |config| {
        toml_table_mut(config, "mcp_servers")?.insert(name, toml::Value::Table(server_table));
        Ok(())
    })
}

pub fn delete_mcp_server_from_codex_config_path(path: &Path, name: &str) -> Result<(), String> {
    update_toml_config(path, |config| {
        if let Some(mcp_servers) = config
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
        {
            mcp_servers.remove(name);
        }
        Ok(())
    })
}

pub fn list_subagents(project_dir: Option<String>) -> Result<Vec<Subagent>, String> {
    let mut subagents = list_subagents_from_dir(&claude_agents_dir()?)?;
    if let Some(project_dir) = project_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let project_agents_dir = project_claude_dir(project_dir)?.join("agents");
        for mut subagent in list_subagents_from_dir(&project_agents_dir)? {
            subagent.scope = SkillScope::Project;
            subagent.id = format!("proj:{}", subagent.id);
            subagents.push(subagent);
        }
    }
    subagents.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(subagents)
}

pub fn list_subagents_from_dir(dir: &Path) -> Result<Vec<Subagent>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut subagents = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取子代理目录失败: {}", e))?
    {
        let entry = entry.map_err(|e| format!("读取子代理目录项失败: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("读取子代理失败: {}", e))?;
        let (meta, prompt) = parse_frontmatter(&content);
        subagents.push(Subagent {
            id: id.clone(),
            name: meta
                .get("x-helm-display-name")
                .cloned()
                .or_else(|| meta.get("display_name").cloned())
                .unwrap_or_else(|| id.clone()),
            model: meta.get("model").cloned().unwrap_or_default(),
            role: meta
                .get("description")
                .cloned()
                .or_else(|| meta.get("role").cloned())
                .unwrap_or_default(),
            tools: meta.get("tools").cloned().unwrap_or_default(),
            auto: meta
                .get("x-helm-auto")
                .map(|value| value == "true")
                .unwrap_or(false),
            prompt,
            scope: SkillScope::Global,
        });
    }
    subagents.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(subagents)
}

pub fn save_subagent(mut subagent: Subagent, project_dir: Option<String>) -> Result<(), String> {
    subagent.id = subagent
        .id
        .strip_prefix("proj:")
        .map(str::to_string)
        .unwrap_or(subagent.id);
    let dir = match subagent.scope {
        SkillScope::Global => claude_agents_dir()?,
        SkillScope::Project => {
            let project_dir = project_dir
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "项目级子代理需要提供项目目录".to_string())?;
            project_claude_dir(project_dir)?.join("agents")
        }
    };
    save_subagent_to_dir(&dir, subagent)
}

pub fn save_subagent_to_dir(dir: &Path, subagent: Subagent) -> Result<(), String> {
    let id = safe_file_stem(if subagent.id.trim().is_empty() {
        &subagent.name
    } else {
        &subagent.id
    })?;
    if subagent.role.trim().is_empty() {
        return Err("子代理职责不能为空".to_string());
    }
    if subagent.prompt.trim().is_empty() {
        return Err("子代理系统提示不能为空".to_string());
    }

    let _guard = shared_config_write_guard()?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建子代理目录失败: {}", e))?;
    let content = markdown_with_frontmatter(
        &[
            ("name", id.clone()),
            ("description", subagent.role),
            ("model", subagent.model),
            ("tools", subagent.tools),
            ("x-helm-display-name", subagent.name),
            ("x-helm-auto", subagent.auto.to_string()),
        ],
        &subagent.prompt,
    );
    write_shared_config_atomically(&dir.join(format!("{id}.md")), content.as_bytes())
        .map_err(|e| format!("写入子代理失败: {e}"))
}

pub fn delete_subagent(id: &str, project_dir: Option<String>) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("proj:") {
        let project_dir = project_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "项目级子代理需要提供项目目录".to_string())?;
        return delete_subagent_from_dir(&project_claude_dir(project_dir)?.join("agents"), id);
    }
    delete_subagent_from_dir(&claude_agents_dir()?, id)
}

pub fn delete_subagent_from_dir(dir: &Path, id: &str) -> Result<(), String> {
    let id = safe_file_stem(id)?;
    let _guard = shared_config_write_guard()?;
    let path = dir.join(format!("{id}.md"));
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("删除子代理失败: {}", e))?;
    }
    Ok(())
}

pub fn list_slash_commands(
    engine: Option<String>,
    cwd: Option<String>,
) -> Result<Vec<SlashCommand>, String> {
    list_slash_commands_from_sources(
        &claude_commands_dir()?,
        Some(&codex_prompts_dir()?),
        cwd.as_deref().map(Path::new),
        engine.as_deref(),
    )
}

/// 旧入口保留给既有测试：只读扩展中心目录，不合并引擎原生来源。
pub fn list_slash_commands_from_dir(
    dir: &Path,
    engine: Option<&str>,
) -> Result<Vec<SlashCommand>, String> {
    list_slash_commands_from_sources(dir, None, None, engine)
}

pub fn list_slash_commands_from_sources(
    extension_dir: &Path,
    codex_prompts_dir: Option<&Path>,
    project_root: Option<&Path>,
    engine: Option<&str>,
) -> Result<Vec<SlashCommand>, String> {
    let mut commands = Vec::new();
    read_slash_commands_from_dir(extension_dir, true, CommandSource::Extension, &mut commands)?;
    read_slash_commands_from_dir(
        &extension_dir.join(".helm-disabled"),
        false,
        CommandSource::Extension,
        &mut commands,
    )?;

    // 引擎原生来源：Codex 用户级 prompts、Claude Code 项目级命令。
    if engine.is_none() || engine == Some("codex") {
        if let Some(dir) = codex_prompts_dir {
            read_codex_prompts_from_dir(dir, &mut commands)?;
        }
    }
    if engine.is_none() || engine == Some("claude-code") {
        if let Some(root) = project_root {
            read_project_claude_commands(&root.join(".claude").join("commands"), &mut commands)?;
        }
    }

    if let Some(engine) = engine {
        commands.retain(|command| command.engine == "all" || command.engine == engine);
        let mut protocol = protocol_slash_commands(Some(engine));
        commands.append(&mut protocol);
        resolve_trigger_conflicts(&mut commands);
    }

    commands.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(commands)
}

/// 同 trigger 冲突裁决：只在按引擎过滤（工作区 `/` 菜单）时执行；
/// 已启用的高优先级来源会遮蔽低优先级项，停用的扩展命令不遮蔽引擎原生命令。
fn resolve_trigger_conflicts(commands: &mut Vec<SlashCommand>) {
    let mut best: HashMap<String, u8> = HashMap::new();
    for command in commands.iter() {
        if !command.enabled {
            continue;
        }
        let trigger = normalize_trigger(&command.trigger);
        let priority = command.source.priority();
        best.entry(trigger)
            .and_modify(|current| *current = (*current).min(priority))
            .or_insert(priority);
    }
    commands.retain(|command| {
        if !command.enabled {
            return true;
        }
        let trigger = normalize_trigger(&command.trigger);
        best.get(&trigger)
            .is_none_or(|&winner| command.source.priority() <= winner)
    });
}

fn read_codex_prompts_from_dir(dir: &Path, commands: &mut Vec<SlashCommand>) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取 Codex prompts 目录失败: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取 Codex prompts 目录项失败: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("读取 Codex prompt 失败: {}", e))?;
        let (meta, body) = parse_frontmatter(&content);
        let description = meta
            .get("description")
            .cloned()
            .unwrap_or_else(|| extract_description(&body));
        commands.push(SlashCommand {
            id: format!("__codex_{stem}"),
            trigger: format!("/{stem}"),
            description,
            scope: SkillScope::Global,
            enabled: true,
            body,
            engine: "codex".to_string(),
            source: CommandSource::EngineUser,
            argument_hint: meta.get("argument-hint").cloned(),
        });
    }
    Ok(())
}

fn read_project_claude_commands(
    dir: &Path,
    commands: &mut Vec<SlashCommand>,
) -> Result<(), String> {
    let mut project = Vec::new();
    read_slash_commands_from_dir(dir, true, CommandSource::EngineProject, &mut project)?;
    read_slash_commands_from_dir(
        &dir.join(".helm-disabled"),
        false,
        CommandSource::EngineProject,
        &mut project,
    )?;
    for mut command in project {
        command.id = format!("__proj_{}", command.id);
        command.engine = "claude-code".to_string();
        command.scope = SkillScope::Project;
        commands.push(command);
    }
    Ok(())
}

fn builtin_command(
    id: &str,
    trigger: &str,
    description: &str,
    body: &str,
    engine: &str,
) -> SlashCommand {
    SlashCommand {
        id: id.to_string(),
        trigger: trigger.to_string(),
        description: description.to_string(),
        scope: SkillScope::Global,
        enabled: true,
        body: body.to_string(),
        engine: engine.to_string(),
        source: CommandSource::Builtin,
        argument_hint: None,
    }
}

fn ui_action_command(trigger: &str, description: &str, action: &str, engine: &str) -> SlashCommand {
    SlashCommand {
        id: format!("__helm_{action}"),
        trigger: trigger.to_string(),
        description: description.to_string(),
        scope: SkillScope::Global,
        enabled: true,
        body: String::new(),
        engine: engine.to_string(),
        source: CommandSource::Builtin,
        argument_hint: None,
    }
}

fn protocol_slash_commands(engine: Option<&str>) -> Vec<SlashCommand> {
    let mut all = Vec::new();

    // 内置命令（变更-08 清理）：
    // - 移除假 /clear——本地展开成一句「请清除上下文」并不会真的清除（还带着 --resume），
    //   属于「口头答应式」命令，清空会话请走「新建会话」；
    // - 移除 /ask、/plan、/build——与发送框的构建/计划/询问模式按钮语义重复且不切换模式，误导。
    let claude: Vec<SlashCommand> = vec![
        builtin_command(
            "__proto_review",
            "/review",
            "审查当前改动",
            "请审查当前改动并给出改进建议。",
            "claude-code",
        ),
        builtin_command(
            "__proto_test",
            "/test",
            "运行相关测试",
            "请运行与当前改动相关的测试。",
            "claude-code",
        ),
        builtin_command(
            "__proto_explain",
            "/explain",
            "解释当前代码或问题",
            "请解释当前代码或问题的原理。",
            "claude-code",
        ),
    ];

    let codex: Vec<SlashCommand> = vec![
        builtin_command(
            "__proto_review",
            "/review",
            "审查当前改动",
            "请审查当前改动并给出改进建议。",
            "codex",
        ),
        builtin_command(
            "__proto_test",
            "/test",
            "运行相关测试",
            "请运行与当前改动相关的测试。",
            "codex",
        ),
    ];

    all.extend(claude);
    all.extend(codex);
    for target in ["claude-code", "codex"] {
        all.extend([
            ui_action_command("/new", "新建会话", "new-session", target),
            ui_action_command("/resume", "打开会话历史", "resume-session", target),
            ui_action_command("/permissions", "打开权限设置", "open-permissions", target),
            ui_action_command("/extensions", "打开扩展中心", "open-extensions", target),
            ui_action_command("/context", "切换上下文面板", "toggle-context", target),
            ui_action_command("/status", "查看当前会话状态", "show-status", target),
            ui_action_command("/stop", "停止当前轮次", "stop-turn", target),
            ui_action_command("/help", "查看命令帮助", "show-help", target),
        ]);
    }

    if let Some(engine) = engine {
        all.retain(|command| command.engine == engine);
    }

    all
}

fn read_slash_commands_from_dir(
    dir: &Path,
    enabled: bool,
    source: CommandSource,
    commands: &mut Vec<SlashCommand>,
) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取斜杠命令目录失败: {}", e))?
    {
        let entry = entry.map_err(|e| format!("读取斜杠命令目录项失败: {}", e))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("读取斜杠命令失败: {}", e))?;
        let (meta, body) = parse_frontmatter(&content);
        let scope = match meta.get("scope").map(String::as_str) {
            Some("project") => SkillScope::Project,
            _ => SkillScope::Global,
        };
        let engine = meta
            .get("x-helm-engine")
            .map(|value| normalize_engine(value))
            .unwrap_or_else(|| "all".to_string());
        commands.push(SlashCommand {
            id: id.clone(),
            trigger: meta
                .get("x-helm-trigger")
                .map(|trigger| normalize_trigger(trigger))
                .unwrap_or_else(|| format!("/{id}")),
            description: meta.get("description").cloned().unwrap_or_default(),
            scope,
            enabled,
            body,
            engine,
            source,
            argument_hint: meta.get("argument-hint").cloned(),
        });
    }
    Ok(())
}

pub fn save_slash_command(
    command: SlashCommand,
    project_dir: Option<String>,
) -> Result<(), String> {
    let project_commands_dir = match project_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(dir) => Some(project_claude_dir(dir)?.join("commands")),
        None => None,
    };
    save_slash_command_routed(
        &claude_commands_dir()?,
        project_commands_dir.as_deref(),
        command,
    )
}

/// 按作用域路由保存：项目级写 `<项目>/.claude/commands`（变更-05：项目级从只读改为可编辑）。
/// 全局旧文件若被改成项目级，写入项目侧成功后移除全局副本（移动语义，避免同名双份）。
pub fn save_slash_command_routed(
    global_dir: &Path,
    project_commands_dir: Option<&Path>,
    mut command: SlashCommand,
) -> Result<(), String> {
    if matches!(
        command.source,
        CommandSource::EngineUser | CommandSource::Builtin
    ) {
        return Err("引擎原生/内置命令为只读，只能编辑扩展中心命令".to_string());
    }
    if command.id.starts_with("__proj_") || command.scope == SkillScope::Project {
        let dir =
            project_commands_dir.ok_or_else(|| "项目级斜杠命令需要提供项目目录".to_string())?;
        let had_project_prefix = command.id.starts_with("__proj_");
        if let Some(stripped) = command.id.strip_prefix("__proj_") {
            command.id = stripped.to_string();
        }
        command.scope = SkillScope::Project;
        command.source = CommandSource::Extension;
        let stem = safe_file_stem(if command.id.trim().is_empty() {
            &command.trigger
        } else {
            &command.id
        })?;
        save_slash_command_to_dir(dir, command)?;
        // 无 __proj_ 前缀说明来自全局列表（旧数据 scope=project 实际存全局）：迁移后清理全局副本
        if !had_project_prefix {
            delete_slash_command_from_dir(global_dir, &stem)?;
        }
        return Ok(());
    }
    save_slash_command_to_dir(global_dir, command)
}

pub fn save_slash_command_to_dir(dir: &Path, command: SlashCommand) -> Result<(), String> {
    if command.source != CommandSource::Extension {
        return Err("引擎原生/内置命令为只读，只能编辑扩展中心命令".to_string());
    }
    let trigger = normalize_trigger(&command.trigger);
    let id = safe_file_stem(if command.id.trim().is_empty() {
        &trigger
    } else {
        &command.id
    })?;
    if command.description.trim().is_empty() {
        return Err("斜杠命令说明不能为空".to_string());
    }
    if command.body.trim().is_empty() {
        return Err("斜杠命令模板不能为空".to_string());
    }

    let _guard = shared_config_write_guard()?;
    let disabled_dir = dir.join(".helm-disabled");
    std::fs::create_dir_all(dir).map_err(|e| format!("创建斜杠命令目录失败: {}", e))?;
    std::fs::create_dir_all(&disabled_dir).map_err(|e| format!("创建禁用命令目录失败: {}", e))?;

    let enabled_path = dir.join(format!("{id}.md"));
    let disabled_path = disabled_dir.join(format!("{id}.md"));
    let mut meta = vec![
        ("description", command.description),
        ("x-helm-trigger", trigger),
        (
            "scope",
            match command.scope {
                SkillScope::Global => "global".to_string(),
                SkillScope::Project => "project".to_string(),
            },
        ),
        ("x-helm-engine", normalize_engine(&command.engine)),
    ];
    if let Some(hint) = command
        .argument_hint
        .as_deref()
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
    {
        meta.push(("argument-hint", hint.to_string()));
    }
    let content = markdown_with_frontmatter(&meta, &command.body);

    if command.enabled {
        write_shared_config_atomically(&enabled_path, content.as_bytes())
            .map_err(|e| format!("写入斜杠命令失败: {e}"))?;
        if disabled_path.exists() {
            std::fs::remove_file(&disabled_path).map_err(|e| format!("移除禁用命令失败: {}", e))?;
        }
        Ok(())
    } else {
        write_shared_config_atomically(&disabled_path, content.as_bytes())
            .map_err(|e| format!("写入禁用斜杠命令失败: {e}"))?;
        if enabled_path.exists() {
            std::fs::remove_file(&enabled_path).map_err(|e| format!("移除启用命令失败: {}", e))?;
        }
        Ok(())
    }
}

pub fn delete_slash_command(id: &str, project_dir: Option<String>) -> Result<(), String> {
    if let Some(stripped) = id.strip_prefix("__proj_") {
        let project_dir = project_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "项目级斜杠命令需要提供项目目录".to_string())?;
        let dir = project_claude_dir(project_dir)?.join("commands");
        return delete_slash_command_from_dir(&dir, stripped);
    }
    delete_slash_command_from_dir(&claude_commands_dir()?, id)
}

pub fn delete_slash_command_from_dir(dir: &Path, id: &str) -> Result<(), String> {
    if id.starts_with("__proto_") || id.starts_with("__codex_") || id.starts_with("__proj_") {
        return Err("引擎原生/内置命令为只读，无法删除".to_string());
    }
    let id = safe_file_stem(id)?;
    let _guard = shared_config_write_guard()?;
    for path in [
        dir.join(format!("{id}.md")),
        dir.join(".helm-disabled").join(format!("{id}.md")),
    ] {
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| format!("删除斜杠命令失败: {}", e))?;
        }
    }
    Ok(())
}

pub fn list_hooks(project_dir: Option<String>) -> Result<Vec<Hook>, String> {
    let mut hooks = list_hooks_from_settings_path(&claude_settings_path()?)?;
    if let Some(project_dir) = project_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let project_settings = project_claude_dir(project_dir)?.join("settings.json");
        for mut hook in list_hooks_from_settings_path(&project_settings)? {
            hook.scope = SkillScope::Project;
            hook.id = format!("proj:{}", hook.id);
            hooks.push(hook);
        }
    }
    hooks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(hooks)
}

pub fn list_hooks_from_settings_path(path: &Path) -> Result<Vec<Hook>, String> {
    let settings = read_settings(path)?;
    let mut hooks_by_id: HashMap<String, Hook> = HashMap::new();

    for key in ["helmHooks", "helmDisabledHooks"] {
        for hook in hook_metadata_from_key(&settings, key) {
            hooks_by_id.insert(hook.id.clone(), hook);
        }
    }

    if let Some(events) = settings.get("hooks").and_then(|value| value.as_object()) {
        for (event_name, matchers) in events {
            let Some(event) = hook_event_from_str(event_name) else {
                continue;
            };
            let Some(matchers) = matchers.as_array() else {
                continue;
            };
            for matcher_config in matchers {
                let match_pattern = matcher_config
                    .get("matcher")
                    .or_else(|| matcher_config.get("match"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("*")
                    .to_string();
                let Some(commands) = matcher_config
                    .get("hooks")
                    .and_then(|value| value.as_array())
                else {
                    continue;
                };
                for command_config in commands {
                    let hook_type = command_config
                        .get("type")
                        .and_then(|value| value.as_str())
                        .unwrap_or("command");
                    if hook_type != "command" {
                        continue;
                    }
                    let Some(command) = command_config
                        .get("command")
                        .and_then(|value| value.as_str())
                    else {
                        continue;
                    };
                    let existing_id = hooks_by_id
                        .iter()
                        .find(|(_, hook)| {
                            hook.event.as_str() == event.as_str()
                                && hook.match_pattern == match_pattern
                                && hook.command == command
                        })
                        .map(|(id, _)| id.clone());
                    let id = existing_id.unwrap_or_else(|| {
                        generated_hook_id(&event, &match_pattern, command)
                            .unwrap_or_else(|_| format!("{}-hook", event.as_str().to_lowercase()))
                    });
                    let mut hook = hooks_by_id.remove(&id).unwrap_or_else(|| Hook {
                        id: id.clone(),
                        event: event.clone(),
                        match_pattern: match_pattern.clone(),
                        command: command.to_string(),
                        description: command.to_string(),
                        enabled: true,
                        scope: SkillScope::Global,
                    });
                    hook.enabled = true;
                    hooks_by_id.insert(id, hook);
                }
            }
        }
    }

    let mut hooks: Vec<Hook> = hooks_by_id.into_values().collect();
    hooks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(hooks)
}

pub fn save_hook(mut hook: Hook, project_dir: Option<String>) -> Result<(), String> {
    hook.id = hook
        .id
        .strip_prefix("proj:")
        .map(str::to_string)
        .unwrap_or(hook.id);
    let path = match hook.scope {
        SkillScope::Global => claude_settings_path()?,
        SkillScope::Project => {
            let project_dir = project_dir
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "项目级钩子需要提供项目目录".to_string())?;
            project_claude_dir(project_dir)?.join("settings.json")
        }
    };
    save_hook_to_settings_path(&path, hook)
}

pub fn save_hook_to_settings_path(path: &Path, mut hook: Hook) -> Result<(), String> {
    let hook_id = if hook.id.trim().is_empty() {
        generated_hook_id(&hook.event, &hook.match_pattern, &hook.command)?
    } else {
        hook.id.clone()
    };
    hook.id = safe_file_stem(&hook_id)?;
    if hook.match_pattern.trim().is_empty() {
        return Err("钩子匹配规则不能为空".to_string());
    }
    if hook.command.trim().is_empty() {
        return Err("钩子命令不能为空".to_string());
    }
    if hook.description.trim().is_empty() {
        hook.description = hook.command.clone();
    }

    update_settings(path, move |settings| {
        let previous = hook_metadata_by_id(settings, &hook.id);
        for old in previous {
            remove_real_hook(settings, &old.event, &old.match_pattern, &old.command)?;
        }
        remove_real_hook(settings, &hook.event, &hook.match_pattern, &hook.command)?;
        remove_hook_metadata(settings, "helmHooks", &hook.id)?;
        remove_hook_metadata(settings, "helmDisabledHooks", &hook.id)?;

        if hook.enabled {
            add_hook_metadata(settings, "helmHooks", &hook)?;
            add_real_hook(settings, &hook)?;
        } else {
            add_hook_metadata(settings, "helmDisabledHooks", &hook)?;
        }
        Ok(())
    })
}

pub fn delete_hook(id: &str, project_dir: Option<String>) -> Result<(), String> {
    if let Some(id) = id.strip_prefix("proj:") {
        let project_dir = project_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "项目级钩子需要提供项目目录".to_string())?;
        let path = project_claude_dir(project_dir)?.join("settings.json");
        return delete_hook_from_settings_path(&path, id);
    }
    delete_hook_from_settings_path(&claude_settings_path()?, id)
}

pub fn delete_hook_from_settings_path(path: &Path, id: &str) -> Result<(), String> {
    let id = safe_file_stem(id)?;
    update_settings(path, move |settings| {
        let previous = hook_metadata_by_id(settings, &id);
        for old in previous {
            remove_real_hook(settings, &old.event, &old.match_pattern, &old.command)?;
        }
        remove_hook_metadata(settings, "helmHooks", &id)?;
        remove_hook_metadata(settings, "helmDisabledHooks", &id)?;
        Ok(())
    })
}

fn hook_event_from_str(event: &str) -> Option<HookEvent> {
    match event {
        "PreToolUse" => Some(HookEvent::PreToolUse),
        "PostToolUse" => Some(HookEvent::PostToolUse),
        "UserPromptSubmit" => Some(HookEvent::UserPromptSubmit),
        "Notification" => Some(HookEvent::Notification),
        "Stop" => Some(HookEvent::Stop),
        "SubagentStop" => Some(HookEvent::SubagentStop),
        "PreCompact" => Some(HookEvent::PreCompact),
        "SessionStart" => Some(HookEvent::SessionStart),
        "SessionEnd" => Some(HookEvent::SessionEnd),
        _ => None,
    }
}

fn generated_hook_id(
    event: &HookEvent,
    match_pattern: &str,
    command: &str,
) -> Result<String, String> {
    safe_file_stem(&format!("{}-{}-{}", event.as_str(), match_pattern, command))
}

fn hook_metadata_from_key(settings: &serde_json::Value, key: &str) -> Vec<Hook> {
    settings
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<Hook>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn hook_metadata_by_id(settings: &serde_json::Value, id: &str) -> Vec<Hook> {
    ["helmHooks", "helmDisabledHooks"]
        .iter()
        .flat_map(|key| hook_metadata_from_key(settings, key))
        .filter(|hook| hook.id == id)
        .collect()
}

fn add_hook_metadata(
    settings: &mut serde_json::Value,
    key: &str,
    hook: &Hook,
) -> Result<(), String> {
    let hooks = array_mut(settings, key)?;
    hooks.push(serde_json::to_value(hook).map_err(|e| format!("序列化钩子失败: {}", e))?);
    Ok(())
}

fn remove_hook_metadata(
    settings: &mut serde_json::Value,
    key: &str,
    id: &str,
) -> Result<(), String> {
    let hooks = array_mut(settings, key)?;
    hooks.retain(|value| value.get("id").and_then(|id_value| id_value.as_str()) != Some(id));
    Ok(())
}

fn add_real_hook(settings: &mut serde_json::Value, hook: &Hook) -> Result<(), String> {
    let hooks_object = object_mut(settings, "hooks")?;
    let event_value = hooks_object
        .entry(hook.event.as_str())
        .or_insert_with(|| serde_json::json!([]));
    if !event_value.is_array() {
        *event_value = serde_json::json!([]);
    }
    let event_hooks = event_value
        .as_array_mut()
        .ok_or_else(|| "hooks 事件配置不是数组".to_string())?;
    event_hooks.push(serde_json::json!({
        "matcher": hook.match_pattern,
        "hooks": [{
            "type": "command",
            "command": hook.command
        }]
    }));
    Ok(())
}

fn remove_real_hook(
    settings: &mut serde_json::Value,
    event: &HookEvent,
    match_pattern: &str,
    command: &str,
) -> Result<(), String> {
    let Some(hooks_object) = settings
        .get_mut("hooks")
        .and_then(|value| value.as_object_mut())
    else {
        return Ok(());
    };
    let Some(event_hooks) = hooks_object
        .get_mut(event.as_str())
        .and_then(|value| value.as_array_mut())
    else {
        return Ok(());
    };

    for matcher_config in event_hooks.iter_mut() {
        let same_match = matcher_config
            .get("matcher")
            .or_else(|| matcher_config.get("match"))
            .and_then(|value| value.as_str())
            .unwrap_or("*")
            == match_pattern;
        if !same_match {
            continue;
        }
        if let Some(commands) = matcher_config
            .get_mut("hooks")
            .and_then(|value| value.as_array_mut())
        {
            commands.retain(|command_config| {
                let hook_type = command_config
                    .get("type")
                    .and_then(|value| value.as_str())
                    .unwrap_or("command");
                let hook_command = command_config
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                !(hook_type == "command" && hook_command == command)
            });
        }
    }

    event_hooks.retain(|matcher_config| {
        matcher_config
            .get("hooks")
            .and_then(|value| value.as_array())
            .map(|commands| !commands.is_empty())
            .unwrap_or(true)
    });
    Ok(())
}

/// 测试 MCP 服务器连接并获取工具列表。
/// 全程 30 秒超时：不回话的 MCP 服务器不允许把前端的 loading 变成永久假死。
pub async fn test_mcp_connection(server: &McpServer) -> Result<Vec<McpTool>, String> {
    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
    match server.transport {
        McpTransport::Stdio => test_stdio_mcp_connection(server, TEST_TIMEOUT).await,
        McpTransport::Sse => tokio::time::timeout(TEST_TIMEOUT, test_sse_mcp_connection(server))
            .await
            .map_err(|_| "MCP 连接测试超时（30 秒无响应）".to_string())?,
        McpTransport::Http => tokio::time::timeout(TEST_TIMEOUT, test_http_mcp_connection(server))
            .await
            .map_err(|_| "MCP 连接测试超时（30 秒无响应）".to_string())?,
    }
}

async fn test_stdio_mcp_connection(
    server: &McpServer,
    test_timeout: std::time::Duration,
) -> Result<Vec<McpTool>, String> {
    let mut command = Command::new(&server.command);
    command
        .args(&server.args)
        .envs(&server.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // 不闪黑框
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 MCP 服务器失败: {}", e))?;

    let stdin = child.stdin.take().ok_or("无法获取 stdin")?;
    let stdout = child.stdout.take().ok_or("无法获取 stdout")?;

    // 阻塞式 JSON-RPC 往返放到独立线程，外层加超时；
    // 超时后杀掉子进程，读端得到 EOF，线程随之退出，不会悬挂。
    let io_task = tokio::task::spawn_blocking(move || stdio_mcp_round_trip(stdin, stdout));
    let result = match tokio::time::timeout(test_timeout, io_task).await {
        Ok(joined) => joined.map_err(|e| format!("MCP 测试线程失败: {e}"))?,
        Err(_) => Err("MCP 连接测试超时（30 秒无响应）".to_string()),
    };

    let _ = child.kill();
    let _ = child.wait();
    result
}

fn stdio_mcp_round_trip(
    mut stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
) -> Result<Vec<McpTool>, String> {
    let reader = BufReader::new(stdout);

    // 发送 initialize 请求
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "Helm",
                "version": "0.1.0"
            }
        }
    });

    writeln!(stdin, "{}", init_request.to_string())
        .map_err(|e| format!("发送 initialize 失败: {}", e))?;

    // 读取 initialize 响应
    let mut lines = reader.lines();
    let _init_response = lines
        .next()
        .ok_or("无响应")?
        .map_err(|e| format!("读取响应失败: {}", e))?;

    // 发送 tools/list 请求
    let tools_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    writeln!(stdin, "{}", tools_request.to_string())
        .map_err(|e| format!("发送 tools/list 失败: {}", e))?;

    // 读取 tools/list 响应
    let tools_response = lines
        .next()
        .ok_or("无工具列表响应")?
        .map_err(|e| format!("读取工具列表失败: {}", e))?;

    let response: serde_json::Value =
        serde_json::from_str(&tools_response).map_err(|e| format!("解析工具列表失败: {}", e))?;
    tools_from_mcp_response(&response)
}

async fn test_sse_mcp_connection(server: &McpServer) -> Result<Vec<McpTool>, String> {
    let url = server.command.trim();
    if url.is_empty() {
        return Err("MCP SSE 服务器必须填写 URL".to_string());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("连接 MCP SSE 服务器失败: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("MCP SSE 连接失败: HTTP {}", response.status()));
    }
    let mut stream = SseStream {
        response,
        buffer: String::new(),
    };
    let endpoint = loop {
        let event = stream.next_event().await?;
        if event.event.as_deref() == Some("endpoint") {
            break event.data.trim().to_string();
        }
    };
    let endpoint_url = resolve_sse_endpoint(url, &endpoint)?;

    post_json_rpc(
        &client,
        &endpoint_url,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "Helm",
                    "version": "0.1.0"
                }
            }
        }),
    )
    .await?;
    read_json_rpc_response(&mut stream, 1).await?;

    post_json_rpc(
        &client,
        &endpoint_url,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await?;

    post_json_rpc(
        &client,
        &endpoint_url,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    )
    .await?;
    let response = read_json_rpc_response(&mut stream, 2).await?;
    tools_from_mcp_response(&response)
}

/// streamable HTTP 传输：initialize / notifications/initialized / tools/list 全部
/// POST 到同一 URL；响应兼容纯 JSON 与 SSE 包裹两种形式，会话头 Mcp-Session-Id 透传。
async fn test_http_mcp_connection(server: &McpServer) -> Result<Vec<McpTool>, String> {
    let url = server.command.trim();
    if url.is_empty() {
        return Err("MCP HTTP 服务器必须填写 URL".to_string());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let mut session_id: Option<String> = None;
    let init_response = post_streamable_http(
        &client,
        url,
        &mut session_id,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "Helm", "version": "0.1.0" }
            }
        }),
    )
    .await?;
    if init_response.is_none() {
        return Err("MCP HTTP initialize 无响应".to_string());
    }

    let _ = post_streamable_http(
        &client,
        url,
        &mut session_id,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await;

    let tools_response = post_streamable_http(
        &client,
        url,
        &mut session_id,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    )
    .await?
    .ok_or("MCP HTTP tools/list 无响应")?;
    tools_from_mcp_response(&tools_response)
}

/// POST 一条 JSON-RPC 消息并解析响应；通知类消息（无 id）返回 Ok(None)。
async fn post_streamable_http(
    client: &reqwest::Client,
    url: &str,
    session_id: &mut Option<String>,
    payload: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let expected_id = payload.get("id").and_then(|id| id.as_i64());
    let mut request = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&payload);
    if let Some(session) = session_id.as_deref() {
        request = request.header("Mcp-Session-Id", session);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("连接 MCP HTTP 服务器失败: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("MCP HTTP 请求失败: HTTP {}", response.status()));
    }
    if let Some(session) = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
    {
        *session_id = Some(session.to_string());
    }
    let Some(expected_id) = expected_id else {
        return Ok(None);
    };
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if content_type.contains("text/event-stream") {
        let mut stream = SseStream {
            response,
            buffer: String::new(),
        };
        loop {
            let event = stream.next_event().await?;
            let value: serde_json::Value = serde_json::from_str(&event.data)
                .map_err(|e| format!("解析 MCP HTTP 响应失败: {}", e))?;
            if value.get("id").and_then(|id| id.as_i64()) == Some(expected_id) {
                return Ok(Some(value));
            }
        }
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("读取 MCP HTTP 响应失败: {}", e))?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("解析 MCP HTTP 响应失败: {}", e))?;
    Ok(Some(value))
}

async fn post_json_rpc(
    client: &reqwest::Client,
    endpoint_url: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    let response = client
        .post(endpoint_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("发送 MCP SSE 请求失败: {}", e))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("发送 MCP SSE 请求失败: HTTP {}", response.status()))
    }
}

async fn read_json_rpc_response(
    stream: &mut SseStream,
    expected_id: i64,
) -> Result<serde_json::Value, String> {
    loop {
        let event = stream.next_event().await?;
        if event.event.as_deref() != Some("message") {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&event.data)
            .map_err(|e| format!("解析 MCP SSE 响应失败: {}", e))?;
        if value.get("id").and_then(|id| id.as_i64()) == Some(expected_id) {
            return Ok(value);
        }
    }
}

fn resolve_sse_endpoint(base_url: &str, endpoint: &str) -> Result<String, String> {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return Ok(endpoint.to_string());
    }
    let base = reqwest::Url::parse(base_url).map_err(|e| format!("MCP SSE URL 无效: {}", e))?;
    base.join(endpoint)
        .map(|url| url.to_string())
        .map_err(|e| format!("MCP SSE endpoint 无效: {}", e))
}

struct SseStream {
    response: reqwest::Response,
    buffer: String,
}

struct SseEvent {
    event: Option<String>,
    data: String,
}

impl SseStream {
    async fn next_event(&mut self) -> Result<SseEvent, String> {
        loop {
            if let Some((block, rest)) = take_sse_block(&self.buffer) {
                self.buffer = rest;
                if let Some(event) = parse_sse_event(&block) {
                    return Ok(event);
                }
                continue;
            }
            let Some(chunk) = self
                .response
                .chunk()
                .await
                .map_err(|e| format!("读取 MCP SSE 流失败: {}", e))?
            else {
                return Err("MCP SSE 连接已关闭".to_string());
            };
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}

fn take_sse_block(buffer: &str) -> Option<(String, String)> {
    let (index, delimiter_len) = match (buffer.find("\r\n\r\n"), buffer.find("\n\n")) {
        (Some(crlf), Some(lf)) if crlf < lf => (crlf, 4),
        (Some(crlf), None) => (crlf, 4),
        (_, Some(lf)) => (lf, 2),
        (None, None) => return None,
    };
    Some((
        buffer[..index].to_string(),
        buffer[index + delimiter_len..].to_string(),
    ))
}

fn parse_sse_event(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data = Vec::new();
    for raw_line in block.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_string());
        }
    }
    (!data.is_empty()).then(|| SseEvent {
        event,
        data: data.join("\n"),
    })
}

fn tools_from_mcp_response(response: &serde_json::Value) -> Result<Vec<McpTool>, String> {
    let tools = response
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .ok_or("响应格式错误")?;

    Ok(tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            McpTool { name, description }
        })
        .collect())
}

// ==================== 技能市场（skills.sh，变更-05） ====================

/// skills.sh 搜索结果条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSkill {
    #[serde(rename = "skillId")]
    pub skill_id: String,
    pub name: String,
    /// 来源 GitHub 仓库，形如 owner/repo
    pub source: String,
    #[serde(default)]
    pub installs: u64,
}

/// 搜索 skills.sh 市场（Agent Skills Directory 公开搜索 API）
pub async fn market_search_skills(query: &str) -> Result<Vec<MarketSkill>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
    let response = client
        .get("https://skills.sh/api/search")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| format!("连接技能市场失败: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("技能市场请求失败: HTTP {}", response.status()));
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("解析技能市场响应失败: {}", e))?;
    parse_market_search_response(&value)
}

pub fn parse_market_search_response(value: &serde_json::Value) -> Result<Vec<MarketSkill>, String> {
    let skills = value
        .get("skills")
        .and_then(|skills| skills.as_array())
        .ok_or("技能市场响应格式错误")?;
    Ok(skills
        .iter()
        .filter_map(|item| {
            let skill_id = item.get("skillId").and_then(|v| v.as_str())?;
            let source = item.get("source").and_then(|v| v.as_str())?;
            Some(MarketSkill {
                skill_id: skill_id.to_string(),
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(skill_id)
                    .to_string(),
                source: source.to_string(),
                installs: item.get("installs").and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .collect())
}

fn validate_market_source(source: &str) -> Result<(), String> {
    let valid = source.split('/').collect::<Vec<_>>().len() == 2
        && source.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' || ch == '/'
        })
        && !source.contains("..");
    if valid {
        Ok(())
    } else {
        Err(format!("技能来源仓库不合法：{source}"))
    }
}

/// SKILL.md 的候选下载 URL：raw.githubusercontent 直连优先，国内镜像兜底；
/// 仓库布局依次尝试 skills/<id>/ → <id>/ → .claude/skills/<id>/，分支 main → master。
pub fn market_download_candidates(source: &str, skill_id: &str) -> Vec<String> {
    const PREFIXES: [&str; 3] = [
        "https://raw.githubusercontent.com",
        "https://ghfast.top/https://raw.githubusercontent.com",
        "https://gh-proxy.com/https://raw.githubusercontent.com",
    ];
    const BRANCHES: [&str; 2] = ["main", "master"];
    let mut urls = Vec::new();
    for prefix in PREFIXES {
        for branch in BRANCHES {
            for path in [
                format!("skills/{skill_id}/SKILL.md"),
                format!("{skill_id}/SKILL.md"),
                format!(".claude/skills/{skill_id}/SKILL.md"),
            ] {
                urls.push(format!("{prefix}/{source}/{branch}/{path}"));
            }
        }
    }
    urls
}

/// 市场安装标记文件内容
pub fn market_marker_json(source: &str, skill_id: &str) -> String {
    serde_json::json!({
        "source": source,
        "skillId": skill_id,
        "installedAt": unix_now(),
    })
    .to_string()
}

/// 从 skills.sh 来源仓库安装技能：下载 SKILL.md 落盘 + 写市场标记。
pub async fn market_install_skill(
    source: &str,
    skill_id: &str,
    scope: SkillScope,
    project_dir: Option<String>,
) -> Result<(), String> {
    validate_market_source(source)?;
    let dir_name = safe_file_stem(skill_id)?;
    let skills_dir = match scope {
        SkillScope::Global => claude_dir()?.join("skills"),
        SkillScope::Project => {
            let project_dir = project_dir
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "安装到项目需要提供项目目录".to_string())?;
            project_claude_dir(project_dir)?.join("skills")
        }
    };

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;
    let mut last_error = String::new();
    let mut content: Option<String> = None;
    for url in market_download_candidates(source, skill_id) {
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => match response.text().await {
                Ok(body) if !body.trim().is_empty() => {
                    content = Some(body);
                    break;
                }
                Ok(_) => last_error = format!("{url} 返回空内容"),
                Err(e) => last_error = format!("{url} 读取失败: {e}"),
            },
            Ok(response) => last_error = format!("{url} 返回 HTTP {}", response.status()),
            Err(e) => last_error = format!("{url} 连接失败: {e}"),
        }
    }
    let content = content.ok_or_else(|| {
        format!("下载 SKILL.md 失败（已尝试直连与镜像），最后一次错误：{last_error}")
    })?;

    let skill_dir = skills_dir.join(&dir_name);
    let _guard = shared_config_write_guard()?;
    std::fs::create_dir_all(&skill_dir).map_err(|e| format!("创建技能目录失败: {}", e))?;
    write_shared_config_atomically(&skill_dir.join("SKILL.md"), content.as_bytes())
        .map_err(|e| format!("写入 SKILL.md 失败: {e}"))?;
    let marker = market_marker_json(source, skill_id);
    write_shared_config_atomically(&skill_dir.join(".helm-market.json"), marker.as_bytes())
        .map_err(|e| format!("写入市场标记失败: {e}"))
}

#[cfg(test)]
mod atomic_write_tests {
    use super::{migrate_legacy_claude_mcp_config_at, write_shared_config_atomically_with};

    #[test]
    fn failure_before_replace_keeps_last_successful_file_and_cleans_temporary_file() {
        let directory = std::env::temp_dir().join(format!(
            "helm-extension-atomic-failure-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        std::fs::write(&path, b"last-successful").unwrap();

        let error = write_shared_config_atomically_with(&path, b"partial-new-value", |_| {
            Err("injected failure".to_string())
        })
        .unwrap_err();

        assert_eq!(error, "injected failure");
        assert_eq!(std::fs::read(&path).unwrap(), b"last-successful");
        let leftovers = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".helm-config-")
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn legacy_claude_mcp_entries_migrate_to_real_cli_config_without_overwriting_target() {
        let directory = std::env::temp_dir().join(format!(
            "helm-extension-mcp-migration-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let legacy_path = directory.join("settings.json");
        let target_path = directory.join(".claude.json");
        std::fs::write(
            &legacy_path,
            r#"{"theme":"dark","mcpServers":{"legacy":{"command":"legacy"},"shared":{"command":"old"}}}"#,
        )
        .unwrap();
        std::fs::write(
            &target_path,
            r#"{"mcpServers":{"shared":{"command":"new"}}}"#,
        )
        .unwrap();

        migrate_legacy_claude_mcp_config_at(&legacy_path, &target_path).unwrap();

        let legacy: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&legacy_path).unwrap()).unwrap();
        let target: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target_path).unwrap()).unwrap();
        assert_eq!(legacy["theme"], "dark");
        assert!(legacy.get("mcpServers").is_none());
        assert_eq!(target["mcpServers"]["legacy"]["command"], "legacy");
        assert_eq!(target["mcpServers"]["shared"]["command"], "new");
        let _ = std::fs::remove_dir_all(directory);
    }
}
