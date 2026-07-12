//! 一键安装 CLI（P3-4，决策 D2 路线 B）：真实执行 `npm install -g <官方包>`。
//!
//! 前置：本机已有 Node.js/npm（没有则给安装指引，不代装 Node——那是路线 C 捆绑的事）。
//! 安装完成后立即复检（where/which + --version），把真实路径与版本返回给前端。

use serde::Serialize;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

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

/// 一键安装引擎 CLI：npm install -g + 安装后复检
#[tauri::command]
pub async fn install_cli_engine(engine: String) -> Result<CliInstallResult, String> {
    let package = npm_package_for_engine(&engine)?;

    if !npm_available().await {
        return Err(
            "未检测到 npm（一键安装需要 Node.js 18+）。请先从 https://nodejs.org 安装 Node.js，\
             或按安装指引在终端手动执行安装命令。"
                .to_string(),
        );
    }

    let mut cmd = npm_command();
    cmd.args(["install", "-g", package]);
    let output = command_output_with_tree_timeout(
        cmd,
        INSTALL_TIMEOUT,
        &format!("安装 {package}（10 分钟）"),
    )
    .await?;
    let tail = output_tail(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(format!(
            "npm install -g {package} 失败（退出码 {}）。常见原因：网络/代理不通、全局目录无写权限。\n{}",
            output.status.code().unwrap_or(-1),
            tail
        ));
    }

    // 安装成功后立即复检，拿真实路径与版本；复检失败说明 PATH 未刷新或安装目录不在 PATH
    let detected = crate::settings::detect_cli_engine(engine.clone()).map_err(|e| {
        format!(
            "安装命令已成功，但复检未找到 {}：{e}。可能需要重启 Helm 让 PATH 生效。",
            engine_executable(&engine)
        )
    })?;

    Ok(CliInstallResult {
        path: detected.path,
        version: detected.version,
        output: tail,
    })
}

#[cfg(test)]
mod tests {
    use super::{command_output_with_tree_timeout, npm_package_for_engine, output_tail};
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
        let escaped_pid_file = pid_file.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$child = Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru; Set-Content -LiteralPath '{}' -Value $child.Id; Start-Sleep -Seconds 30",
            escaped_pid_file
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);

        let error = command_output_with_tree_timeout(cmd, Duration::from_millis(1500), "测试命令")
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
