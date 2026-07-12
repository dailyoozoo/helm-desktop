// 文件快照存储：在写操作前保存文件当前内容，用于检查点回溯。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: String,
    #[serde(default = "default_existed")]
    pub existed: bool,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub files: Vec<FileSnapshot>,
}

fn default_existed() -> bool {
    true
}

pub struct SnapshotStore {
    snapshots_dir: PathBuf,
}

impl SnapshotStore {
    pub fn new(snapshots_dir: PathBuf) -> Self {
        Self { snapshots_dir }
    }

    pub fn save(&self, checkpoint_id: &str, snapshot: &Snapshot) -> Result<(), String> {
        fs::create_dir_all(&self.snapshots_dir).map_err(|e| format!("创建快照目录失败: {}", e))?;
        let path = self.snapshots_dir.join(format!("{}.json", checkpoint_id));
        let json =
            serde_json::to_string_pretty(snapshot).map_err(|e| format!("序列化快照失败: {}", e))?;
        crate::providers::write_atomically(&path, &json)
            .map_err(|e| format!("写入快照文件失败: {}", e))?;
        Ok(())
    }

    pub fn load(&self, checkpoint_id: &str) -> Result<Snapshot, String> {
        let path = self.snapshots_dir.join(format!("{}.json", checkpoint_id));
        let json = fs::read_to_string(&path).map_err(|e| format!("读取快照文件失败: {}", e))?;
        let snapshot: Snapshot =
            serde_json::from_str(&json).map_err(|e| format!("解析快照文件失败: {}", e))?;
        Ok(snapshot)
    }

    pub fn restore_files(&self, snapshot: &Snapshot) -> Result<(), String> {
        for file in &snapshot.files {
            let path = Path::new(&file.path);
            if !file.existed {
                if path.exists() {
                    fs::remove_file(path)
                        .map_err(|e| format!("删除新增文件失败 {}: {}", path.display(), e))?;
                }
                continue;
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
            }
            fs::write(path, &file.content)
                .map_err(|e| format!("恢复文件失败 {}: {}", path.display(), e))?;
        }
        Ok(())
    }

    pub fn delete(&self, checkpoint_id: &str) -> Result<(), String> {
        let path = self.snapshots_dir.join(format!("{}.json", checkpoint_id));
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("删除快照文件失败: {}", e))?;
        }
        Ok(())
    }

    pub fn capture_files(&self, paths: &[PathBuf]) -> Result<Snapshot, String> {
        let mut files = Vec::new();
        for path in paths {
            if path.exists() {
                let content = fs::read_to_string(path)
                    .map_err(|e| format!("读取文件失败 {}: {}", path.display(), e))?;
                files.push(FileSnapshot {
                    path: path.to_string_lossy().to_string(),
                    existed: true,
                    content,
                });
            } else {
                files.push(FileSnapshot {
                    path: path.to_string_lossy().to_string(),
                    existed: false,
                    content: String::new(),
                });
            }
        }
        Ok(Snapshot { files })
    }
}

#[cfg(test)]
mod tests {
    use super::SnapshotStore;
    use std::fs;

    fn test_dir(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("helm-snapshot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn restore_removes_file_that_did_not_exist_when_captured() {
        let root = test_dir("missing-file");
        let file_path = root.join("created-by-agent.txt");
        let store = SnapshotStore::new(root.join("snapshots"));

        let snapshot = store.capture_files(&[file_path.clone()]).unwrap();
        fs::write(&file_path, "new content").unwrap();

        store.restore_files(&snapshot).unwrap();

        assert!(!file_path.exists());
        let _ = fs::remove_dir_all(root);
    }
}
