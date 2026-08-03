use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct WorkspaceExecutionCoordinator {
    active: Arc<Mutex<Vec<ActiveWorkspace>>>,
}

#[derive(Debug)]
struct ActiveWorkspace {
    session_id: String,
    path: PathBuf,
}

#[derive(Debug)]
pub struct WorkspaceExecutionLease {
    coordinator: WorkspaceExecutionCoordinator,
    session_id: String,
    path: PathBuf,
}

impl WorkspaceExecutionCoordinator {
    pub fn acquire(
        &self,
        session_id: impl Into<String>,
        cwd: impl AsRef<Path>,
    ) -> Result<WorkspaceExecutionLease, String> {
        let session_id = session_id.into();
        let path = cwd.as_ref().canonicalize().map_err(|error| {
            format!(
                "工作目录不可用，无法启动构建：{}：{error}",
                cwd.as_ref().display()
            )
        })?;
        if !path.is_dir() {
            return Err(format!("工作目录不是文件夹：{}", path.display()));
        }

        let mut active = self
            .active
            .lock()
            .map_err(|_| "工作目录执行协调器锁中毒".to_string())?;
        if let Some(conflict) = active
            .iter()
            .find(|entry| paths_overlap(&entry.path, &path))
        {
            return Err(format!(
                "[workspace_busy] 另一个构建会话正在使用相同或重叠的工作目录：{}（会话 {}）。请等待该轮次结束后重试。",
                conflict.path.display(),
                conflict.session_id
            ));
        }
        active.push(ActiveWorkspace {
            session_id: session_id.clone(),
            path: path.clone(),
        });
        drop(active);

        Ok(WorkspaceExecutionLease {
            coordinator: self.clone(),
            session_id,
            path,
        })
    }
}

impl Drop for WorkspaceExecutionLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.coordinator.active.lock() {
            active.retain(|entry| {
                entry.session_id != self.session_id || !same_path(&entry.path, &self.path)
            });
        }
    }
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    is_ancestor_or_same(left, right) || is_ancestor_or_same(right, left)
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalized_components(left) == normalized_components(right)
}

fn is_ancestor_or_same(parent: &Path, child: &Path) -> bool {
    let parent = normalized_components(parent);
    let child = normalized_components(child);
    parent.len() <= child.len() && parent.iter().zip(child.iter()).all(|(a, b)| a == b)
}

fn normalized_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let value = component.as_os_str().to_string_lossy().to_string();
            #[cfg(windows)]
            {
                value.to_lowercase()
            }
            #[cfg(not(windows))]
            {
                value
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::WorkspaceExecutionCoordinator;
    use std::path::PathBuf;

    fn tempdir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "helm-workspace-execution-{name}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn rejects_same_directory_until_lease_is_released() {
        let root = tempdir("same");
        let coordinator = WorkspaceExecutionCoordinator::default();
        let lease = coordinator.acquire("one", &root).unwrap();

        assert!(coordinator.acquire("two", &root).is_err());
        drop(lease);
        assert!(coordinator.acquire("two", &root).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_parent_child_overlap_in_both_directions() {
        let root = tempdir("nested");
        let child = root.join("child");
        std::fs::create_dir(&child).unwrap();
        let coordinator = WorkspaceExecutionCoordinator::default();

        let parent_lease = coordinator.acquire("parent", &root).unwrap();
        assert!(coordinator.acquire("child", &child).is_err());
        drop(parent_lease);

        let child_lease = coordinator.acquire("child", &child).unwrap();
        assert!(coordinator.acquire("parent", &root).is_err());
        drop(child_lease);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn permits_disjoint_directories() {
        let root = tempdir("disjoint");
        let left = root.join("left");
        let right = root.join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let coordinator = WorkspaceExecutionCoordinator::default();

        let _left = coordinator.acquire("left", &left).unwrap();
        assert!(coordinator.acquire("right", &right).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_directory() {
        let root = tempdir("missing");
        let coordinator = WorkspaceExecutionCoordinator::default();

        assert!(coordinator
            .acquire("missing", root.join("missing"))
            .is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
