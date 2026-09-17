//! 登录 shell PATH 探测。
//!
//! Agent 的 Bash 工具默认走 MSYS bash 解释命令，bash 看到的 PATH 才是 Agent
//! 真实有效的搜索清单。Helm 进程从桌面快捷方式启动时继承的是 Windows 注册表
//! PATH——往往缺 `Git\usr\bin`（which.exe 在这）、缺真实 Python（只有 WindowsApps
//! 存根），导致 Agent 的 bash 报 `python: command not found` / `which: command
//! not found`，退路全断。
//!
//! 通用做法：启动时跑一次 `bash -lc 'echo "$PATH"'`，把结果作为「追加到 PATH
//! 末尾」的目录列表。这样不写死任何路径，换台电脑 / Git 装在 D 盘 / 根本没装
//! Git，都自然成立。失败时（找不到 bash / bash 卡住 / 输出非 PATH 格式）返回
//! None，调用方保持原 PATH，零行为变化。
//!
//! 安全：不动白名单机制，仅在白名单过滤后把探测到的目录去重追加到 PATH 末尾，
//! 优先级最低，不覆盖任何已有条目。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_millis(2000);

static LOGIN_SHELL_PATH: OnceLock<Option<String>> = OnceLock::new();

/// 返回登录 shell 探测到的 PATH 字符串（MSYS 格式：`:/-分隔`）。
/// 整个 Helm 进程只探测一次。失败时返回 None。
pub fn detect_login_shell_path() -> Option<&'static str> {
    LOGIN_SHELL_PATH
        .get_or_init(probe_inner)
        .as_deref()
}

/// 把 `extra` 里的目录去重追加到 `env` 中 PATH 值的末尾（不动其它变量）。
/// 找不到 PATH 键时追加一条新的。Windows 用 `;`，Unix 用 `:` 分隔。
pub fn merge_path_extra(env: &mut Vec<(String, String)>, extra: &str) {
    let sep = path_separator();
    for (k, v) in env.iter_mut() {
        if k.eq_ignore_ascii_case("PATH") {
            *v = merge_path_string(v, extra, sep);
            return;
        }
    }
    env.push(("PATH".to_string(), extra.to_string()));
}

fn path_separator() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

fn merge_path_string(existing: &str, extra: &str, sep: char) -> String {
    let mut seen: HashSet<String> = HashSet::new();
    let mut parts: Vec<String> = Vec::new();
    for raw in existing.split(sep).chain(extra.split(sep)) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            parts.push(trimmed.to_string());
        }
    }
    parts.join(&sep.to_string())
}

fn probe_inner() -> Option<String> {
    let (prog, args) = probe_command()?;
    let out = run_with_timeout(&prog, &args, PROBE_TIMEOUT)?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(windows)]
fn probe_command() -> Option<(String, Vec<&'static str>)> {
    let bash = find_bash_windows()?;
    Some((bash, vec!["-lc", "printf '%s' \"$PATH\""]))
}

#[cfg(not(windows))]
fn probe_command() -> Option<(String, Vec<&'static str>)> {
    for prog in &["bash", "sh", "zsh"] {
        if program_works(prog) {
            return Some(((*prog).to_string(), vec!["-lc", "printf '%s' \"$PATH\""]));
        }
    }
    None
}

#[cfg(not(windows))]
fn program_works(prog: &str) -> bool {
    Command::new(prog)
        .arg("-c")
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn find_bash_windows() -> Option<String> {
    // 1) where.exe bash —— Git for Windows 默认会注册到 PATH
    if let Some(p) = where_first_exe("bash") {
        if std::path::Path::new(&p).is_file() {
            return Some(p);
        }
    }
    // 2) Git for Windows 注册表安装路径（不假设盘符）
    if let Some(install) = git_install_location_from_registry() {
        for sub in &["bin\\bash.exe", "usr\\bin\\bash.exe"] {
            let p = install.join(sub);
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    // 3) 兜底：常见路径
    for c in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        if std::path::Path::new(c).is_file() {
            return Some(c.to_string());
        }
    }
    None
}

#[cfg(windows)]
fn where_first_exe(name: &str) -> Option<String> {
    let out = Command::new("where.exe")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(windows)]
fn git_install_location_from_registry() -> Option<PathBuf> {
    // 64-bit 与 32-bit 视图都要试；Git for Windows 安装时写 HKLM。
    let entries: [(&str, &str); 2] = [
        (r"HKLM\SOFTWARE\GitForWindows", "InstallPath"),
        (r"HKLM\SOFTWARE\WOW6432Node\GitForWindows", "InstallPath"),
    ];
    for (key, value) in entries {
        if let Some(path) = reg_query_string(key, value) {
            let p = PathBuf::from(path);
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(windows)]
fn reg_query_string(key: &str, value: &str) -> Option<String> {
    // 走 reg.exe 简单可靠，不引入 winreg 依赖。
    let out = Command::new("reg.exe")
        .args(["query", key, "/v", value])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // 输出形如：
    //   HKEY_LOCAL_MACHINE\SOFTWARE\GitForWindows
    //       InstallPath    REG_SZ    C:\Program Files\Git
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(value) {
            // 跳过 value 名和 REG_SZ 类型，取第三列起的真实值
            if let Some(pos) = trimmed.find("REG_SZ") {
                let rest = trimmed[pos + "REG_SZ".len()..].trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

fn run_with_timeout(prog: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::sync::mpsc;
    use std::thread;
    let (tx, rx) = mpsc::channel();
    let prog = prog.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let _handle = thread::spawn(move || {
        let result = Command::new(&prog)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_unique_to_existing_path() {
        let mut env = vec![("PATH".to_string(), "/a;/b".to_string())];
        merge_path_extra(&mut env, "/b;/c");
        assert_eq!(env[0].1, "/a;/b;/c");
    }

    #[test]
    fn merge_preserves_existing_order() {
        let mut env = vec![("PATH".to_string(), "/a;/b".to_string())];
        merge_path_extra(&mut env, "/c");
        assert_eq!(env[0].1, "/a;/b;/c");
    }

    #[test]
    fn merge_handles_case_insensitive_path_key() {
        let mut env = vec![("path".to_string(), "/a".to_string())];
        merge_path_extra(&mut env, "/b");
        assert_eq!(env[0].1, "/a;/b");
    }

    #[test]
    fn merge_skips_empty_segments() {
        let mut env = vec![("PATH".to_string(), "/a;;/b".to_string())];
        merge_path_extra(&mut env, ";/c;;");
        assert_eq!(env[0].1, "/a;/b;/c");
    }

    #[test]
    fn merge_adds_path_when_missing() {
        let mut env = vec![("HOME".to_string(), "/home".to_string())];
        merge_path_extra(&mut env, "/x;/y");
        assert_eq!(env.len(), 2);
        assert_eq!(env[1], ("PATH".to_string(), "/x;/y".to_string()));
    }

    #[test]
    fn merge_path_string_unit() {
        let merged = merge_path_string("/a;/b", "/b;/c", ';');
        assert_eq!(merged, "/a;/b;/c");
    }

    #[test]
    fn detect_login_shell_path_is_cached() {
        // 同一个进程内多次调用必须返回相同引用（OnceLock 语义）。
        let a = detect_login_shell_path();
        let b = detect_login_shell_path();
        assert_eq!(a.is_some(), b.is_some());
        if let (Some(pa), Some(pb)) = (a, b) {
            assert_eq!(pa.as_ptr(), pb.as_ptr());
        }
    }
}
