//! worktree 隔离（P3-3）契约测试：跑真实 `git`，验证创建/命名安全/删除防线。

use helm_lib::worktree::{prepare_worktree, remove_worktree, resolve_worktree_root};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_repo(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("helm-worktree-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git 可执行");
    assert!(
        output.status.success(),
        "git {args:?} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo_with_commit(path: &Path) {
    git(path, &["init", "-q"]);
    git(path, &["config", "user.email", "helm-test@example.com"]);
    git(path, &["config", "user.name", "Helm Test"]);
    fs::write(path.join("README.md"), "# demo\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-q", "-m", "init"]);
}

#[tokio::test]
async fn worktree_prepare_creates_isolated_checkout_and_branch() {
    let repo = temp_repo("prepare");
    init_repo_with_commit(&repo);

    let info = prepare_worktree(&repo, "feature-a", "", "")
        .await
        .expect("创建 worktree");

    let worktree_path = Path::new(&info.path);
    assert!(worktree_path.is_dir(), "worktree 目录必须真实存在");
    assert!(
        worktree_path.join("README.md").is_file(),
        "worktree 必须是仓库的完整检出"
    );
    assert_eq!(info.branch, "helm/feature-a");
    // 默认根目录在仓库旁边
    let root = resolve_worktree_root(&repo, "").unwrap();
    assert!(worktree_path.starts_with(&root));

    // 分支真实存在
    let branches = Command::new("git")
        .args(["branch", "--list", "helm/feature-a"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&branches.stdout).contains("helm/feature-a"));

    // 清理
    remove_worktree(&repo, worktree_path, "").await.unwrap();
    assert!(!worktree_path.exists(), "删除后目录必须消失");
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn worktree_rejects_non_git_directory_with_actionable_error() {
    let dir = temp_repo("not-git");
    let err = prepare_worktree(&dir, "x", "", "").await.unwrap_err();
    assert!(err.contains("不是 Git 仓库"), "错误要说明原因：{err}");
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn worktree_remove_refuses_paths_outside_configured_root() {
    let repo = temp_repo("remove-guard");
    init_repo_with_commit(&repo);
    // 拿仓库自己当删除目标：不在 worktree 根内，必须拒绝
    let err = remove_worktree(&repo, &repo, "")
        .await
        .expect_err("必须拒绝根外路径");
    assert!(err.contains("拒绝删除") || err.contains("不存在"), "{err}");
    assert!(repo.exists(), "原仓库不能被碰");
    let _ = fs::remove_dir_all(&repo);
}

#[tokio::test]
async fn worktree_name_is_sanitized_and_collisions_get_suffixed() {
    let repo = temp_repo("sanitize");
    init_repo_with_commit(&repo);

    let first = prepare_worktree(&repo, "修复 登录/bug!", "", "")
        .await
        .expect("非法字符要被清洗而不是失败");
    assert!(
        !first.path.contains('!') && !first.path.contains('/') || Path::new(&first.path).is_dir()
    );

    // 同名重复创建不覆盖，自动加后缀
    let a = prepare_worktree(&repo, "same", "", "").await.unwrap();
    let b = prepare_worktree(&repo, "same", "", "").await.unwrap();
    assert_ne!(a.path, b.path, "重名必须得到不同目录");

    let root = resolve_worktree_root(&repo, "").unwrap();
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn worktree_setup_script_runs_inside_new_worktree() {
    let repo = temp_repo("setup");
    init_repo_with_commit(&repo);

    #[cfg(windows)]
    let script = "echo setup-ok> setup-marker.txt";
    #[cfg(not(windows))]
    let script = "echo setup-ok > setup-marker.txt";

    let info = prepare_worktree(&repo, "with-setup", "", script)
        .await
        .expect("带 setup 脚本创建");
    assert!(
        Path::new(&info.path).join("setup-marker.txt").is_file(),
        "setup 脚本必须真实在 worktree 内执行"
    );

    let root = resolve_worktree_root(&repo, "").unwrap();
    remove_worktree(&repo, Path::new(&info.path), "")
        .await
        .unwrap();
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&root);
}
