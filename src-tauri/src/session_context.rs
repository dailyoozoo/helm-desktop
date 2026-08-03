use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSessionContext {
    pub kind: &'static str,
    pub canonical_path: String,
    pub canonical_path_digest: String,
    pub identity_digest: String,
    pub display_name: String,
}

pub fn validate_session_context_path(
    cwd: &str,
    source_path: &str,
) -> Result<ValidatedSessionContext, String> {
    let source = Path::new(source_path);
    if source_path.trim().is_empty() || !source.is_absolute() {
        return Err("会话上下文必须使用绝对路径".to_string());
    }
    reject_unsafe_path_shape(source)?;
    reject_linked_components(source)?;

    let canonical_cwd = Path::new(cwd)
        .canonicalize()
        .map_err(|error| format!("无法解析会话工作目录：{error}"))?;
    let canonical = source.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "会话上下文路径不存在或已被移动".to_string()
        } else {
            format!("无法解析会话上下文路径：{error}")
        }
    })?;
    if !path_is_within(&canonical_cwd, &canonical) {
        return Err("当前版本只允许加入会话工作目录内的上下文".to_string());
    }
    let metadata =
        fs::metadata(&canonical).map_err(|error| format!("无法读取会话上下文元数据：{error}"))?;
    let kind = if metadata.is_file() {
        reject_multiple_hard_links(&canonical, &metadata)?;
        "file"
    } else if metadata.is_dir() {
        "directory"
    } else {
        return Err("会话上下文只支持普通文件或目录".to_string());
    };
    let canonical_path = canonical.to_string_lossy().to_string();
    if crate::permissions::sensitive_path_is_denied(&canonical_path) {
        return Err("敏感路径不能加入会话上下文".to_string());
    }
    let identity_digest = file_identity_digest(&canonical, &metadata, kind)?;
    let display_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&canonical_path)
        .to_string();
    Ok(ValidatedSessionContext {
        kind,
        canonical_path_digest: digest(canonical_path.as_bytes()),
        canonical_path,
        identity_digest,
        display_name,
    })
}

fn reject_unsafe_path_shape(path: &Path) -> Result<(), String> {
    let raw = path.to_string_lossy().replace('\\', "/");
    if raw.starts_with("//") || raw.starts_with("//?/") || raw.starts_with("//./") {
        return Err("UNC、设备路径或扩展路径不能加入会话上下文".to_string());
    }
    if raw
        .split('/')
        .any(|component| component.ends_with('.') || component.ends_with(' '))
    {
        return Err("包含尾点或尾空格别名的路径不能加入会话上下文".to_string());
    }
    #[cfg(windows)]
    {
        for component in path.components() {
            let Component::Normal(value) = component else {
                continue;
            };
            let value = value.to_string_lossy();
            if value.contains(':') {
                return Err("NTFS ADS 路径不能加入会话上下文".to_string());
            }
            let stem = value
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_end_matches(['.', ' '])
                .to_ascii_uppercase();
            if matches!(
                stem.as_str(),
                "CON"
                    | "PRN"
                    | "AUX"
                    | "NUL"
                    | "COM1"
                    | "COM2"
                    | "COM3"
                    | "COM4"
                    | "COM5"
                    | "COM6"
                    | "COM7"
                    | "COM8"
                    | "COM9"
                    | "LPT1"
                    | "LPT2"
                    | "LPT3"
                    | "LPT4"
                    | "LPT5"
                    | "LPT6"
                    | "LPT7"
                    | "LPT8"
                    | "LPT9"
            ) {
                return Err("Windows 保留设备名不能加入会话上下文".to_string());
            }
        }
    }
    Ok(())
}

fn reject_linked_components(path: &Path) -> Result<(), String> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                cursor.push(component.as_os_str());
            }
            Component::CurDir | Component::ParentDir => {
                return Err("包含路径别名的路径不能加入会话上下文".to_string());
            }
        }
        if matches!(component, Component::Normal(_)) {
            let metadata = fs::symlink_metadata(&cursor).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "会话上下文路径不存在或已被移动".to_string()
                } else {
                    format!("无法校验会话上下文路径：{error}")
                }
            })?;
            if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
                return Err("symlink、junction 或 reparse path 不能加入会话上下文".to_string());
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn reject_multiple_hard_links(path: &Path, _: &fs::Metadata) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err("无法验证会话上下文 hard-link 身份".to_string());
    }
    let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    let read = unsafe { GetFileInformationByHandle(handle, &mut info) };
    unsafe { CloseHandle(handle) };
    if read == 0 {
        return Err("无法验证会话上下文 hard-link 身份".to_string());
    }
    if info.nNumberOfLinks > 1 {
        return Err("多 hard-link 文件不能加入会话上下文".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn reject_multiple_hard_links(_: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if metadata.nlink() > 1 {
        return Err("多 hard-link 文件不能加入会话上下文".to_string());
    }
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn reject_multiple_hard_links(_: &Path, _: &fs::Metadata) -> Result<(), String> {
    Ok(())
}

fn path_is_within(root: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        let root = root
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>();
        let target = target
            .components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>();
        target.starts_with(&root)
    }
    #[cfg(not(windows))]
    {
        target.starts_with(root)
    }
}

fn file_identity_digest(
    path: &Path,
    metadata: &fs::Metadata,
    kind: &str,
) -> Result<String, String> {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let identity = serde_json::to_vec(&serde_json::json!({
        "pathDigest": digest(path.to_string_lossy().as_bytes()),
        "kind": kind,
        "length": metadata.len(),
        "modifiedNanos": modified,
    }))
    .map_err(|error| format!("生成上下文身份摘要失败：{error}"))?;
    Ok(digest(&identity))
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_workspace_file_and_rejects_outside_and_sensitive_paths() {
        let root = std::env::temp_dir().join(format!("helm-context-{}", rand::random::<u64>()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/app.txt"), "hello").unwrap();
        fs::write(root.join(".env"), "SECRET=value").unwrap();
        let outside = std::env::temp_dir().join(format!("helm-outside-{}", rand::random::<u64>()));
        fs::write(&outside, "outside").unwrap();

        let valid = validate_session_context_path(
            &root.to_string_lossy(),
            &root.join("src/app.txt").to_string_lossy(),
        )
        .unwrap();
        assert_eq!(valid.kind, "file");
        assert!(valid.identity_digest.starts_with("sha256:"));
        assert!(
            validate_session_context_path(&root.to_string_lossy(), &outside.to_string_lossy())
                .is_err()
        );
        assert!(validate_session_context_path(
            &root.to_string_lossy(),
            &root.join(".env").to_string_lossy()
        )
        .is_err());

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(outside);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_source_path_that_contains_a_symlink_before_canonicalization() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("helm-context-link-{}", rand::random::<u64>()));
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real/context.txt"), "hello").unwrap();
        symlink(root.join("real"), root.join("linked")).unwrap();

        let error = validate_session_context_path(
            &root.to_string_lossy(),
            &root.join("linked/context.txt").to_string_lossy(),
        )
        .unwrap_err();
        assert!(error.contains("symlink"));

        let _ = fs::remove_dir_all(root);
    }
}
