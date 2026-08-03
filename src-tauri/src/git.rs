//! Git 只读查询命令。
//!
//! 提供三个只读 git 命令：获取当前分支名、工作区状态、暂存区文件列表。
//! 全部使用 `std::process::Command` 调用 git CLI，与现有探针风格一致，不引入 git2 crate。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Git 工作区状态摘要
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// 当前分支名（detached HEAD 时为 commit hash 前 7 位）
    pub branch: String,
    /// 已修改文件数
    pub modified: usize,
    /// 新增文件数（未跟踪）
    pub added: usize,
    /// 已删除文件数
    pub deleted: usize,
}

/// 暂存区文件条目
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StagedFile {
    /// 文件路径（相对于 cwd）
    pub path: String,
    /// 变更类型：Added / Modified / Deleted / Renamed
    pub status: String,
}

/// 执行 git 命令并返回 stdout；失败时返回错误信息。
fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("执行 git 失败：{e}"))?;

    if output.status.success() {
        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_string())
            .map_err(|e| format!("git 输出编码错误：{e}"))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git {} 失败：{}", args.join(" "), stderr.trim()))
    }
}

/// 获取当前分支名。
/// - 正常分支返回分支名
/// - detached HEAD 返回 commit hash 前 7 位
#[tauri::command]
pub async fn get_git_branch(cwd: String) -> Result<String, String> {
    let path = Path::new(&cwd);
    if !path.is_dir() {
        return Err(format!("目录不存在：{cwd}"));
    }

    // 尝试获取分支名
    match run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(branch) => {
            if branch == "HEAD" {
                // detached HEAD，返回 commit hash 前 7 位
                run_git(path, &["rev-parse", "--short=7", "HEAD"])
            } else {
                Ok(branch)
            }
        }
        Err(e) => Err(e),
    }
}

/// 获取工作区状态：modified / added / deleted 文件数。
///
/// 使用 `git status --porcelain` 解析：
/// - `M` / `T` = modified
/// - `A` / `??` = added（未跟踪也算 added）
/// - `D` = deleted
#[tauri::command]
pub async fn get_git_status(cwd: String) -> Result<GitStatus, String> {
    let path = Path::new(&cwd);
    if !path.is_dir() {
        return Err(format!("目录不存在：{cwd}"));
    }

    let branch = get_git_branch(cwd.clone())
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    let porcelain = run_git(path, &["status", "--porcelain"])?;

    let mut modified = 0usize;
    let mut added = 0usize;
    let mut deleted = 0usize;

    for line in porcelain.lines() {
        if line.len() < 2 {
            continue;
        }
        let index_status = line.as_bytes()[0];
        let worktree_status = line.as_bytes()[1];

        // 未跟踪文件
        if index_status == b'?' && worktree_status == b'?' {
            added += 1;
            continue;
        }

        // 工作区变更
        match worktree_status {
            b'M' | b'T' => modified += 1,
            b'D' => deleted += 1,
            _ => {}
        }

        // 暂存区变更（如果工作区没有变更）
        if worktree_status == b' ' {
            match index_status {
                b'M' | b'T' => modified += 1,
                b'A' => added += 1,
                b'D' => deleted += 1,
                _ => {}
            }
        }
    }

    Ok(GitStatus {
        branch,
        modified,
        added,
        deleted,
    })
}

/// 获取暂存区文件列表。
///
/// 使用 `git diff --cached --name-status` 解析。
#[tauri::command]
pub async fn get_git_staged(cwd: String) -> Result<Vec<StagedFile>, String> {
    let path = Path::new(&cwd);
    if !path.is_dir() {
        return Err(format!("目录不存在：{cwd}"));
    }

    let output = run_git(path, &["diff", "--cached", "--name-status"])?;

    let files: Vec<StagedFile> = output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, '\t').collect();
            if parts.len() == 2 {
                Some(StagedFile {
                    status: parts[0].trim().to_string(),
                    path: parts[1].trim().to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(files)
}
