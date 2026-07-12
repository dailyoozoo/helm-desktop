//! Git worktree 隔离（P3-3）：并行会话可在独立 worktree 中运行，互不踩踏工作区。
//!
//! 避坑设计（可靠性检查 §3 设计要求）：
//! - 可关闭：`settings.worktree.enabled = false` 时 UI 不出现该选项；
//! - 位置可配：默认在仓库旁边的 `<仓库名>-worktrees/`，可在设置里改根目录；
//! - setup 脚本机制：worktree 创建后可自动跑一条初始化命令（如 npm install）；
//! - 不绑死 GitHub PR 流：只做本地 worktree/分支，不做任何远端操作。

use crate::sessions::SessionHistoryStore;
use crate::settings::load_app_settings_from_store;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::State;
use tokio::process::Command;

const GIT_TIMEOUT: Duration = Duration::from_secs(60);
const SETUP_SCRIPT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    /// setup 脚本输出尾部（没配脚本则为空）
    pub setup_output: String,
}

fn git_command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    #[cfg(windows)]
    {
        // 不闪黑框（与引擎检测一致）
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git_command(cwd);
    cmd.args(args);
    let output = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("git {} 超时（60s）", args.join(" ")))?
        .map_err(|e| format!("执行 git 失败（未安装或不在 PATH）：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} 失败：{}",
            args.join(" "),
            stderr.trim().chars().take(400).collect::<String>()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 目录是否在 git 仓库工作区内
pub async fn is_git_worktree(cwd: &Path) -> bool {
    run_git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .await
        .map(|out| out == "true")
        .unwrap_or(false)
}

/// worktree 名字只允许安全字符，避免拼进路径/分支名出问题
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("wt-{}", now_millis())
    } else {
        trimmed
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// worktree 根目录：设置里配了就用配置；否则默认放在仓库旁边 `<仓库名>-worktrees/`
pub fn resolve_worktree_root(base_cwd: &Path, configured_root: &str) -> Result<PathBuf, String> {
    let configured = configured_root.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    let repo_name = base_cwd
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法从工作目录推断仓库名".to_string())?;
    let parent = base_cwd
        .parent()
        .ok_or_else(|| "工作目录没有上级目录，无法放置 worktree".to_string())?;
    Ok(parent.join(format!("{repo_name}-worktrees")))
}

/// 创建一个隔离 worktree（真实 `git worktree add -b`），可选执行 setup 脚本
pub async fn prepare_worktree(
    base_cwd: &Path,
    name: &str,
    configured_root: &str,
    setup_script: &str,
) -> Result<WorktreeInfo, String> {
    if !base_cwd.is_dir() {
        return Err(format!("工作目录不存在：{}", base_cwd.display()));
    }
    if !is_git_worktree(base_cwd).await {
        return Err(format!(
            "{} 不是 Git 仓库；worktree 隔离需要先在该目录 git init 或选择一个仓库目录",
            base_cwd.display()
        ));
    }
    // 仓库必须至少有一个提交，否则无法创建分支
    if run_git(base_cwd, &["rev-parse", "HEAD"]).await.is_err() {
        return Err("仓库还没有任何提交，无法创建 worktree（先完成一次 git commit）".to_string());
    }

    let name = sanitize_name(name);
    let root = resolve_worktree_root(base_cwd, configured_root)?;
    std::fs::create_dir_all(&root).map_err(|e| format!("创建 worktree 根目录失败：{e}"))?;
    let mut path = root.join(&name);
    let mut branch = format!("helm/{name}");
    // 重名时追加时间戳，避免覆盖
    if path.exists() {
        let suffixed = format!("{name}-{}", now_millis());
        path = root.join(&suffixed);
        branch = format!("helm/{suffixed}");
    }

    run_git(
        base_cwd,
        &[
            "worktree",
            "add",
            path.to_string_lossy().as_ref(),
            "-b",
            &branch,
        ],
    )
    .await?;

    let mut setup_output = String::new();
    let script = setup_script.trim();
    if !script.is_empty() {
        setup_output = run_setup_script(&path, script).await.unwrap_or_else(|err| {
            // setup 失败不回滚 worktree：目录可用，用户可自行修复依赖
            format!("[setup 脚本失败] {err}")
        });
    }

    Ok(WorktreeInfo {
        path: path.to_string_lossy().to_string(),
        branch,
        setup_output,
    })
}

async fn run_setup_script(worktree: &Path, script: &str) -> Result<String, String> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(script);
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    };
    cmd.current_dir(worktree);
    let output = crate::installer::command_output_with_tree_timeout(
        cmd,
        SETUP_SCRIPT_TIMEOUT,
        "setup 脚本（10 分钟）",
    )
    .await?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let tail: String = combined
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    if !output.status.success() {
        return Err(format!(
            "setup 脚本退出码 {}：{}",
            output.status.code().unwrap_or(-1),
            tail
        ));
    }
    Ok(tail)
}

/// 删除 helm 创建的 worktree。安全约束：目标必须位于解析出的 worktree 根目录之下，
/// 防止把任意目录当 worktree 删掉。
pub async fn remove_worktree(
    base_cwd: &Path,
    worktree_path: &Path,
    configured_root: &str,
) -> Result<(), String> {
    let root = resolve_worktree_root(base_cwd, configured_root)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|_| format!("worktree 根目录不存在：{}", root.display()))?;
    let canonical_target = worktree_path
        .canonicalize()
        .map_err(|_| format!("worktree 目录不存在：{}", worktree_path.display()))?;
    if !canonical_target.starts_with(&canonical_root) || canonical_target == canonical_root {
        return Err(format!(
            "拒绝删除：{} 不在 worktree 根目录 {} 之内",
            worktree_path.display(),
            root.display()
        ));
    }
    run_git(
        base_cwd,
        &[
            "worktree",
            "remove",
            "--force",
            canonical_target.to_string_lossy().as_ref(),
        ],
    )
    .await?;
    Ok(())
}

// ============ Tauri 命令 ============

/// 用应用设置里的 worktree 配置创建隔离 worktree
#[tauri::command]
pub async fn create_session_worktree(
    history_store: State<'_, SessionHistoryStore>,
    base_cwd: String,
    name: String,
) -> Result<WorktreeInfo, String> {
    let settings = load_app_settings_from_store(&history_store)?;
    if !settings.worktree.enabled {
        return Err("worktree 隔离已在设置中关闭".to_string());
    }
    prepare_worktree(
        Path::new(&base_cwd),
        &name,
        &settings.worktree.root,
        &settings.worktree.setup_script,
    )
    .await
}

/// 删除 helm 创建的 worktree（只允许删 worktree 根目录内的路径）
#[tauri::command]
pub async fn remove_session_worktree(
    history_store: State<'_, SessionHistoryStore>,
    base_cwd: String,
    worktree_path: String,
) -> Result<(), String> {
    let settings = load_app_settings_from_store(&history_store)?;
    remove_worktree(
        Path::new(&base_cwd),
        Path::new(&worktree_path),
        &settings.worktree.root,
    )
    .await
}
