//! 一键安装 CLI 与环境依赖（P3-4 + 变更-37）：真实执行 `npm install -g <官方包>`，
//! 官方源失败自动切 `registry.npmmirror.com` 兜底；并负责 Node / git 的环境探测与一键安装。
//!
//! 前置：CLI 安装需要本机已有 Node.js/npm；npm 不可用时返回「先装 Node」引导
//! （`install_node` 会用国内镜像下载 Node LTS 静默安装，`install_git` 下载 git-for-windows）。
//! 安装完成后立即复检（where/which + --version），把真实路径与版本返回给前端。

use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// 首次按用户已有 registry 安装的等待上限（网络不通时尽快切镜像，不干等满超时）
const NPM_DEFAULT_REGISTRY_TIMEOUT: Duration = Duration::from_secs(240);

/// 国内 npm 镜像（官方源失败兜底）
const NPM_MIRROR_REGISTRY: &str = "https://registry.npmmirror.com";
/// Node.js 国内镜像（npmmirror 二进制镜像，等价 npmmirror.com/mirrors/node）
const NODE_MIRROR_DIR: &str = "https://registry.npmmirror.com/-/binary/node";
/// git-for-windows 国内二进制镜像
const GIT_MIRROR_DIR: &str = "https://registry.npmmirror.com/-/binary/git-for-windows";

const DOWNLOAD_MAX_BYTES: u64 = 128 * 1024 * 1024;
const LISTING_MAX_BYTES: u64 = 4 * 1024 * 1024;

async fn terminate_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .output()
            .await;
    }
    #[cfg(not(windows))]
    {
        let group = format!("-{pid}");
        let _ = Command::new("kill").args(["-TERM", &group]).output().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = Command::new("kill").args(["-KILL", &group]).output().await;
    }
}

pub(crate) async fn command_output_with_tree_timeout(
    mut cmd: Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| format!("执行{label}失败：{e}"))?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(result) => result.map_err(|e| format!("等待{label}失败：{e}")),
        Err(_) => {
            terminate_process_tree(pid).await;
            Err(format!("{label}超时，已终止相关进程"))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallResult {
    pub path: String,
    pub version: String,
    /// npm 输出尾部（诊断用）
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInstallResult {
    /// 安装后复检到的可执行文件路径（PATH 或已知安装目录）
    pub path: String,
    pub version: String,
    /// true 表示 PATH 尚未刷新，新进程（包括重启 Helm 前）仍解析不到，需要重启 Helm
    pub restart_required: bool,
}

/// 工作区共享依赖（Node/npm/git）探测结果：真实 `--version`，可用才标 true。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDepStatus {
    pub available: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDeps {
    pub node: WorkspaceDepStatus,
    pub npm: WorkspaceDepStatus,
    pub git: WorkspaceDepStatus,
}

/// 探测单个可执行文件的可用性与版本（真实 `--version`，失败一律视为缺失，不猜测）。
async fn probe_version(program: &str, via_cmd: bool) -> WorkspaceDepStatus {
    let mut cmd = probe_command(program, via_cmd);
    cmd.arg("--version");
    match command_output_with_tree_timeout(cmd, Duration::from_secs(30), &format!("检测 {program}"))
        .await
    {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string)
                .filter(|version| !version.is_empty());
            WorkspaceDepStatus {
                available: true,
                version,
            }
        }
        _ => WorkspaceDepStatus {
            available: false,
            version: None,
        },
    }
}

fn probe_command(program: &str, via_cmd: bool) -> Command {
    #[cfg(windows)]
    {
        if via_cmd {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(program);
            cmd.creation_flags(0x0800_0000);
            return cmd;
        }
        let mut cmd = Command::new(program);
        cmd.creation_flags(0x0800_0000);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// 探测共享依赖：node / npm / git。npm 在 Windows 上是 npm.cmd，必须经 cmd /C 走 PATH 解析。
#[tauri::command]
pub async fn detect_workspace_deps() -> Result<WorkspaceDeps, String> {
    let node = probe_version("node", false).await;
    let npm = probe_version("npm", true).await;
    let git = probe_version("git", false).await;
    Ok(WorkspaceDeps { node, npm, git })
}

fn npm_package_for_engine(engine: &str) -> Result<&'static str, String> {
    match engine {
        "claude-code" => Ok("@anthropic-ai/claude-code"),
        "codex" => Ok("@openai/codex"),
        other => Err(format!("未知引擎：{other}")),
    }
}

fn engine_executable(engine: &str) -> &'static str {
    if engine == "codex" {
        "codex"
    } else {
        "claude"
    }
}

/// npm 是否可用（Windows 上 npm 是 npm.cmd，必须经 cmd /C 走 PATH 解析）
async fn npm_available() -> bool {
    let mut cmd = npm_command();
    cmd.arg("--version");
    matches!(
        command_output_with_tree_timeout(cmd, Duration::from_secs(30), "npm 检测").await,
        Ok(output) if output.status.success()
    )
}

fn npm_command() -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("cmd");
        c.arg("/C").arg("npm");
        c.creation_flags(0x0800_0000);
        c
    }
    #[cfg(not(windows))]
    {
        Command::new("npm")
    }
}

fn output_tail(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    combined
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(15)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// CLI 安装的尝试序列：第一次尊重用户已有 registry（默认即官方源），失败后切 npmmirror 兜底。
/// 抽成纯函数便于单测锁定「尊重 registry → 镜像兜底」的顺序。
fn npm_install_attempts() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("默认 registry（尊重已有配置）", None),
        ("国内镜像 npmmirror", Some(NPM_MIRROR_REGISTRY)),
    ]
}

/// 一键安装引擎 CLI：npm install -g + 安装后复检。
/// 官方源失败自动切 `registry.npmmirror.com` 兜底；npm 不可用时引导先装 Node。
#[tauri::command]
pub async fn install_cli_engine(engine: String) -> Result<CliInstallResult, String> {
    let package = npm_package_for_engine(&engine)?;

    if !npm_available().await {
        return Err(
            "未检测到 npm（一键安装需要 Node.js 18+）。请在引导卡一键安装 Node.js，\
             或先从 https://nodejs.org 安装 Node.js 后重试。"
                .to_string(),
        );
    }

    let attempts = npm_install_attempts();
    let mut first_error: Option<String> = None;
    for (index, (label, registry)) in attempts.iter().enumerate() {
        let mut cmd = npm_command();
        cmd.args(["install", "-g", package]);
        if let Some(registry) = registry {
            cmd.args(["--registry", registry]);
        }
        let timeout = if index == 0 {
            NPM_DEFAULT_REGISTRY_TIMEOUT
        } else {
            INSTALL_TIMEOUT
        };
        let output = match command_output_with_tree_timeout(
            cmd,
            timeout,
            &format!("安装 {package}（{label}）"),
        )
        .await
        {
            Ok(output) => output,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error.clone());
                }
                continue;
            }
        };
        let tail = output_tail(&output.stdout, &output.stderr);
        if !output.status.success() {
            if first_error.is_none() {
                first_error = Some(format!(
                    "退出码 {}：{}",
                    output.status.code().unwrap_or(-1),
                    tail
                ));
            }
            continue;
        }

        // 安装成功后立即复检，拿真实路径与版本；复检失败说明 PATH 未刷新或安装目录不在 PATH
        let detected = crate::settings::detect_cli_engine(engine.clone()).map_err(|e| {
            format!(
                "安装命令已成功，但复检未找到 {}：{e}。可能需要重启 Helm 让 PATH 生效。",
                engine_executable(&engine)
            )
        })?;
        return Ok(CliInstallResult {
            path: detected.path,
            version: detected.version,
            output: tail,
        });
    }

    Err(format!(
        "npm install -g {package} 失败（默认源与国内镜像均已尝试）。\
         常见原因：网络受限、全局目录无写权限。\n{}",
        first_error.unwrap_or_default()
    ))
}

/// 下载文件到内存（带大小上限与真实状态码校验，禁止 mock）。
async fn download_bytes(url: &str, max_bytes: u64, label: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建下载客户端失败：{e}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载{label}失败：{e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载{label}失败：HTTP {}",
            response.status().as_u16()
        ));
    }
    if response.content_length().unwrap_or_default() > max_bytes {
        return Err(format!("{label}超过大小上限"));
    }
    let mut output = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|_| format!("读取{label}失败"))?
    {
        if output.len().saturating_add(chunk.len()) as u64 > max_bytes {
            return Err(format!("{label}超过大小上限"));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn setup_cache_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("helm-setup");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
    Ok(dir)
}

/// 解析 npmmirror 二进制镜像目录列表：优先 JSON（`[{name,type}]`），失败回退 HTML `<a href>`。
fn parse_binary_dir_listing(body: &str) -> Vec<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(entries) = value.as_array() {
            let names: Vec<String> = entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("name")
                        .and_then(|name| name.as_str())
                        .map(str::to_string)
                })
                .collect();
            if !names.is_empty() {
                return names;
            }
        }
    }
    let re = Regex::new(r#"href="([^"/]+)/?""#).expect("静态正则");
    re.captures_iter(body)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// 解析版本号中的数字段（忽略 `windows`、`rc` 等非数字段），返回可字典序比较的序列。
fn parse_version_parts(version: &str) -> Vec<u32> {
    version
        .trim_start_matches('v')
        .split(['.', '-'])
        .filter_map(|part| part.parse::<u32>().ok())
        .collect()
}

fn select_latest_version(names: &[String]) -> Option<String> {
    names
        .iter()
        .filter(|name| name.starts_with('v'))
        .max_by_key(|name| parse_version_parts(name))
        .cloned()
}

/// 从 nodejs dist index.json 挑选最新 LTS 版本（`lts` 为真值字符串的版本里版本号最高的）。
fn select_latest_lts(index_json: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(index_json).map_err(|e| format!("解析 Node 版本索引失败：{e}"))?;
    let entries = value.as_array().ok_or("Node 版本索引格式错误")?;
    let mut best: Option<(Vec<u32>, String)> = None;
    for entry in entries {
        let Some(version) = entry.get("version").and_then(|v| v.as_str()) else {
            continue;
        };
        let is_lts = entry
            .get("lts")
            .and_then(|v| v.as_str())
            .map(|name| !name.is_empty())
            .unwrap_or(false);
        if !is_lts {
            continue;
        }
        let parts = parse_version_parts(version);
        if best
            .as_ref()
            .map(|(best_parts, _)| parts > *best_parts)
            .unwrap_or(true)
        {
            best = Some((parts, version.to_string()));
        }
    }
    best.map(|(_, version)| version)
        .ok_or_else(|| "Node 版本索引中没有可用的 LTS 版本".to_string())
}

/// 用 SHA-256SUMS 校验下载产物；找不到条目或哈希不匹配一律 fail-closed。
fn verify_sha256(contents: &[u8], shasums_text: &str, target_file: &str) -> Result<(), String> {
    let expected = shasums_text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let file = parts.next()?;
            (file.trim_start_matches('*') == target_file)
                .then(|| hash.to_string().to_ascii_lowercase())
        })
        .next()
        .ok_or_else(|| format!("校验文件中找不到 {target_file}"))?;
    let actual = crate::util::sha256_hex(contents);
    if actual != expected {
        return Err(format!("{target_file} SHA-256 校验失败，安装已中止"));
    }
    Ok(())
}

/// 复检已安装工具：优先 PATH 定位（重启前即生效）；失败则检查已知安装目录并标记需要重启。
async fn recheck_installed_tool(
    program: &str,
    known_locations: &[PathBuf],
    label: &str,
) -> Result<ToolInstallResult, String> {
    let mut which_cmd = Command::new(if cfg!(windows) { "where" } else { "which" });
    which_cmd.arg(program);
    #[cfg(windows)]
    {
        which_cmd.creation_flags(0x0800_0000);
    }
    if let Ok(output) = command_output_with_tree_timeout(
        which_cmd,
        Duration::from_secs(30),
        &format!("定位 {label}"),
    )
    .await
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !path.is_empty() {
                if let Ok((path, version)) = probe_file_version(Path::new(&path)) {
                    return Ok(ToolInstallResult {
                        path,
                        version,
                        restart_required: false,
                    });
                }
            }
        }
    }
    for candidate in known_locations {
        if let Ok((path, version)) = probe_file_version(candidate) {
            return Ok(ToolInstallResult {
                path,
                version,
                restart_required: true,
            });
        }
    }
    Err(format!(
        "{label} 安装命令已执行，但复检未找到 {program}。可能需要重启 Helm 让 PATH 生效，或安装需要管理员权限。"
    ))
}

fn probe_file_version(path: &Path) -> Result<(String, String), String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{path:?}"));
    }
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| format!("执行 {path:?} 失败：{e}"))?;
    if !output.status.success() {
        return Err(format!("{path:?} --version 失败"));
    }
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown")
        .to_string();
    Ok((path.to_string_lossy().to_string(), version))
}

#[cfg(windows)]
fn node_known_locations() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(PathBuf::from(program_files).join("nodejs").join("node.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Programs")
                .join("nodejs")
                .join("node.exe"),
        );
    }
    candidates
}

#[cfg(windows)]
fn git_known_locations() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Git")
                .join("cmd")
                .join("git.exe"),
        );
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Git")
                .join("cmd")
                .join("git.exe"),
        );
    }
    candidates
}

/// 一键安装 Node LTS：国内镜像下载 MSI → SHA-256SUMS 验签 → msiexec 静默安装（per-user 优先）→ 复检。
#[tauri::command]
pub async fn install_node() -> Result<ToolInstallResult, String> {
    #[cfg(windows)]
    {
        let index = download_bytes(
            &format!("{NODE_MIRROR_DIR}/index.json"),
            LISTING_MAX_BYTES,
            "Node 版本索引",
        )
        .await?;
        let version = select_latest_lts(&String::from_utf8_lossy(&index))?;
        let msi_name = format!("node-{version}-x64.msi");
        let msi_url = format!("{NODE_MIRROR_DIR}/{version}/{msi_name}");
        let shasums_url = format!("{NODE_MIRROR_DIR}/{version}/SHASUMS256.txt");

        let msi = download_bytes(&msi_url, DOWNLOAD_MAX_BYTES, "Node.js 安装包").await?;
        let shasums = download_bytes(&shasums_url, 256 * 1024, "Node.js 校验文件").await?;
        verify_sha256(&msi, &String::from_utf8_lossy(&shasums), &msi_name)?;

        let msi_path = setup_cache_dir()?.join(&msi_name);
        std::fs::write(&msi_path, &msi).map_err(|e| format!("写入安装包失败：{e}"))?;

        let mut cmd = Command::new("msiexec");
        cmd.arg("/i")
            .arg(&msi_path)
            .arg("/quiet")
            .arg("/norestart")
            .arg("ALLUSERS=2")
            .arg("MSIINSTALLPERUSER=1");
        cmd.creation_flags(0x0800_0000);
        let output = command_output_with_tree_timeout(cmd, INSTALL_TIMEOUT, "安装 Node.js").await?;
        let _ = std::fs::remove_file(&msi_path);
        if !output.status.success() {
            return Err(format!(
                "Node.js 静默安装失败（退出码 {}）。常见原因：安装包需要管理员权限或系统策略限制。\n{}",
                output.status.code().unwrap_or(-1),
                output_tail(&output.stdout, &output.stderr)
            ));
        }
        recheck_installed_tool("node", &node_known_locations(), "Node.js").await
    }
    #[cfg(not(windows))]
    {
        Err("当前平台暂不支持一键安装 Node.js，请使用系统包管理器（如 brew / apt）安装".to_string())
    }
}

/// 一键安装 git：国内二进制镜像下载 git-for-windows 64 位安装包 → SHA-256 校验 → 静默安装 → 复检。
#[tauri::command]
pub async fn install_git() -> Result<ToolInstallResult, String> {
    #[cfg(windows)]
    {
        let root_body = download_bytes(
            &format!("{GIT_MIRROR_DIR}/"),
            LISTING_MAX_BYTES,
            "git 版本目录",
        )
        .await?;
        let root_text = String::from_utf8_lossy(&root_body).to_string();
        let versions = parse_binary_dir_listing(&root_text);
        let latest =
            select_latest_version(&versions).ok_or("git-for-windows 镜像中没有可用版本")?;

        let folder_body = download_bytes(
            &format!("{GIT_MIRROR_DIR}/{latest}/"),
            LISTING_MAX_BYTES,
            "git 安装包目录",
        )
        .await?;
        let folder_text = String::from_utf8_lossy(&folder_body).to_string();
        let files = parse_binary_dir_listing(&folder_text);

        let exe_name = files
            .iter()
            .find(|name| name.ends_with("-64-bit.exe"))
            .cloned()
            .ok_or("git 镜像中找不到 64 位安装包")?;
        let exe_url = format!("{GIT_MIRROR_DIR}/{latest}/{exe_name}");
        let exe = download_bytes(&exe_url, DOWNLOAD_MAX_BYTES, "Git 安装包").await?;

        let sums_name = files
            .iter()
            .find(|name| name.to_ascii_lowercase().contains("sha256sum"))
            .cloned()
            .ok_or("git 镜像不提供 SHA-256 校验文件，已中止安装")?;
        let sums_url = format!("{GIT_MIRROR_DIR}/{latest}/{sums_name}");
        let sums = download_bytes(&sums_url, 512 * 1024, "Git 校验文件").await?;
        verify_sha256(&exe, &String::from_utf8_lossy(&sums), &exe_name)?;

        let exe_path = setup_cache_dir()?.join(&exe_name);
        std::fs::write(&exe_path, &exe).map_err(|e| format!("写入安装包失败：{e}"))?;

        let mut cmd = Command::new(&exe_path);
        cmd.arg("/VERYSILENT")
            .arg("/NORESTART")
            .arg("/SP-")
            .arg("/NOCANCEL")
            .arg("/CURRENTUSER");
        cmd.creation_flags(0x0800_0000);
        let output = command_output_with_tree_timeout(cmd, INSTALL_TIMEOUT, "安装 Git").await?;
        let _ = std::fs::remove_file(&exe_path);
        if !output.status.success() {
            return Err(format!(
                "Git 静默安装失败（退出码 {}）。\n{}",
                output.status.code().unwrap_or(-1),
                output_tail(&output.stdout, &output.stderr)
            ));
        }
        recheck_installed_tool("git", &git_known_locations(), "Git").await
    }
    #[cfg(not(windows))]
    {
        Err("git 一键安装仅支持 Windows；其他平台请使用系统包管理器安装".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        command_output_with_tree_timeout, npm_package_for_engine, output_tail,
        parse_binary_dir_listing, parse_version_parts, select_latest_lts, select_latest_version,
        verify_sha256, NPM_MIRROR_REGISTRY,
    };
    use std::time::Duration;
    use tokio::process::Command;

    #[test]
    fn engine_maps_to_official_npm_packages() {
        assert_eq!(
            npm_package_for_engine("claude-code").unwrap(),
            "@anthropic-ai/claude-code"
        );
        assert_eq!(npm_package_for_engine("codex").unwrap(), "@openai/codex");
        assert!(npm_package_for_engine("unknown").is_err());
    }

    #[test]
    fn output_tail_keeps_last_lines_and_drops_blanks() {
        let stdout = (1..=30)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = output_tail(stdout.as_bytes(), b"\n\nerr-final\n");
        assert!(tail.ends_with("err-final"));
        assert!(!tail.contains("line-1\n"), "只保留尾部");
        assert!(tail.lines().count() <= 15);
    }

    #[test]
    fn npm_install_attempts_respects_registry_then_falls_back_to_mirror() {
        let attempts = super::npm_install_attempts();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].0, "默认 registry（尊重已有配置）");
        assert!(
            attempts[0].1.is_none(),
            "首次不附加 --registry，尊重用户已有配置"
        );
        assert_eq!(
            attempts[1].1,
            Some(NPM_MIRROR_REGISTRY),
            "官方源失败切 npmmirror 兜底"
        );
    }

    #[test]
    fn parse_binary_dir_listing_parses_json_and_html() {
        let json = r#"[{"name":"v2.45.1.windows.1","type":"dir"},{"name":"v2.44.0.windows.2","type":"dir"}]"#;
        assert_eq!(
            parse_binary_dir_listing(json),
            vec![
                "v2.45.1.windows.1".to_string(),
                "v2.44.0.windows.2".to_string()
            ]
        );
        let html = r#"<a href="v2.45.1.windows.1/">v2.45.1.windows.1/</a>"#;
        assert_eq!(
            parse_binary_dir_listing(html),
            vec!["v2.45.1.windows.1".to_string()]
        );
    }

    #[test]
    fn version_parts_ignore_non_numeric_suffix() {
        assert_eq!(parse_version_parts("v22.14.0"), vec![22, 14, 0]);
        assert_eq!(parse_version_parts("v2.45.1.windows.1"), vec![2, 45, 1, 1]);
        assert_eq!(parse_version_parts("v22.0.0-rc.1"), vec![22, 0, 0, 1]);
    }

    #[test]
    fn select_latest_version_picks_newest_git_tag() {
        let names = vec![
            "v2.44.0.windows.2".to_string(),
            "v2.45.1.windows.1".to_string(),
            "README".to_string(),
        ];
        assert_eq!(
            select_latest_version(&names).as_deref(),
            Some("v2.45.1.windows.1")
        );
        assert!(select_latest_version(&["README".to_string()]).is_none());
    }

    #[test]
    fn select_latest_lts_skips_current_and_prefers_highest_lts() {
        let index = serde_json::json!([
            { "version": "v24.3.0", "lts": "Krypton" },
            { "version": "v23.5.0", "lts": false },
            { "version": "v22.14.0", "lts": "Jod" },
            { "version": "v20.19.0", "lts": "Iron" }
        ]);
        let selected = select_latest_lts(&index.to_string()).unwrap();
        assert_eq!(
            selected, "v24.3.0",
            "选版本号最高的 LTS，跳过 current(false)"
        );
    }

    #[test]
    fn select_latest_lts_fails_without_any_lts() {
        let index = serde_json::json!([
            { "version": "v23.5.0", "lts": false },
            { "version": "v22.14.0", "lts": "" }
        ]);
        assert!(select_latest_lts(&index.to_string()).is_err());
    }

    #[test]
    fn sha256_verify_accepts_matching_and_rejects_tampered() {
        let contents = b"node-msi-bytes";
        let hash = crate::util::sha256_hex(contents);
        let shasums = format!(
            "3c5e76fbb5e00a51d71d3f078f79ab3fbbd8f8a4ec6e4a1c9a12d3f9c9f3b0e9  node-other.msi\n{hash} *node-v22.14.0-x64.msi\n"
        );
        assert!(verify_sha256(contents, &shasums, "node-v22.14.0-x64.msi").is_ok());
        assert!(verify_sha256(b"tampered", &shasums, "node-v22.14.0-x64.msi").is_err());
        assert!(verify_sha256(contents, &shasums, "not-listed.msi").is_err());
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn process_tree_timeout_helper() {
        let Some(pid_file) = std::env::var_os("HELM_INSTALLER_TIMEOUT_PID_FILE") else {
            return;
        };
        let mut child = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .spawn()
            .unwrap();
        std::fs::write(pid_file, child.id().to_string()).unwrap();
        let _ = child.wait();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_terminates_spawned_process_tree() {
        let pid_file = std::env::temp_dir().join(format!(
            "helm-timeout-child-{}-{}.pid",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--ignored",
            "--exact",
            "installer::tests::process_tree_timeout_helper",
        ])
        .env("HELM_INSTALLER_TIMEOUT_PID_FILE", &pid_file);

        let error = command_output_with_tree_timeout(cmd, Duration::from_secs(15), "测试命令")
            .await
            .expect_err("命令应超时");
        assert!(error.contains("超时"));

        let child_pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("父进程应在超时前写出子进程 PID")
            .trim()
            .parse()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let alive = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("Get-Process -Id {child_pid} -ErrorAction SilentlyContinue"),
            ])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&alive.stdout).trim().is_empty(),
            "超时后不应遗留子进程 {child_pid}"
        );
        let _ = std::fs::remove_file(pid_file);
    }
}
