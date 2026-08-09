//! 文件/附件预览命令（变更-33）。
//!
//! 提供两条只读命令：
//! - `read_file_preview`：读取文件内容用于软件内预览。文本返回 UTF-8 内容，图片返回 base64
//!   数据，二进制返回类型标记。全部经过敏感路径拒绝与大小上限保护，只读不写。
//! - `open_path_in_system`：用系统默认程序打开一个**已存在**的文件/目录（接入 opener，
//!   供二进制/非图片文件外置打开）。同样经过敏感路径拒绝。

use base64::Engine;
use opener::open as open_with_system;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 文本预览最大字节数；超过按该大小截断，避免把超大日志灌进 UI。
const MAX_TEXT_BYTES: u64 = 256 * 1024;
/// 图片预览最大字节数（base64 再膨胀约 1.33 倍）。
const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

/// 预览内容类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PreviewKind {
    /// UTF-8 文本，`content` 为已脱敏文本
    Text,
    /// 图片，`content` 为 data URL（不含前缀依赖，由前端拼 MIME）
    Image,
    /// 二进制或其他不可文本化内容，无内容字段；前端应引导用户走系统默认程序打开
    Binary,
}

/// 文件预览结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub kind: PreviewKind,
    /// Text 时为文本内容；Image 时为原始图片 Bytes 的 base64
    pub content: Option<String>,
    /// 图片 MIME（image/png 等）；非图片为 None
    pub mime: Option<String>,
    /// 实际文件字节数
    pub size: u64,
    /// 是否因超过文本预览上限而被截断
    pub truncated: bool,
}

fn sensitive_path(path: &Path) -> bool {
    crate::permissions::sensitive_path_is_denied(&path.to_string_lossy())
}

fn is_image_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("svg") => Some("image/svg+xml"),
        _ => None,
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(2048).any(|byte| *byte == 0)
}

fn read_sized(path: &Path, limit: u64) -> Result<(Vec<u8>, u64, bool), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取文件元信息失败：{e}"))?;
    let size = meta.len();
    if size > limit {
        return Ok((Vec::new(), size, true));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败：{e}"))?;
    Ok((bytes, size, false))
}

/// 读取任意允许文件用于软件内预览。
#[tauri::command]
pub fn read_file_preview(path: String) -> Result<FilePreview, String> {
    let resolved = PathBuf::from(&path);
    if sensitive_path(&resolved) {
        return Err("该路径属于系统敏感目录，Helm 拒绝预览".to_string());
    }
    if !resolved.is_file() {
        return Err(format!("文件不存在或不是普通文件：{path}"));
    }

    if let Some(mime) = is_image_path(&resolved) {
        let (bytes, size, truncated) = read_sized(&resolved, MAX_IMAGE_BYTES)?;
        if truncated {
            return Ok(FilePreview {
                kind: PreviewKind::Image,
                content: None,
                mime: Some(mime.to_string()),
                size,
                truncated: true,
            });
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Ok(FilePreview {
            kind: PreviewKind::Image,
            content: Some(encoded),
            mime: Some(mime.to_string()),
            size,
            truncated: false,
        });
    }

    let (bytes, size, truncated) = read_sized(&resolved, MAX_TEXT_BYTES)?;
    if truncated {
        return Ok(FilePreview {
            kind: PreviewKind::Binary,
            content: None,
            mime: None,
            size,
            truncated: true,
        });
    }
    if looks_binary(&bytes) {
        return Ok(FilePreview {
            kind: PreviewKind::Binary,
            content: None,
            mime: None,
            size,
            truncated: false,
        });
    }
    let raw = String::from_utf8(bytes).map_err(|_| "文件不是有效的 UTF-8 文本".to_string())?;
    let content = crate::redaction::redact_text(&raw);
    Ok(FilePreview {
        kind: PreviewKind::Text,
        content: Some(content),
        mime: None,
        size,
        truncated: false,
    })
}

/// 用系统默认程序打开文件/目录（供二进制或需要在外部查看的文件使用）。
#[tauri::command]
pub fn open_path_in_system(path: String) -> Result<(), String> {
    let resolved = PathBuf::from(&path);
    if sensitive_path(&resolved) {
        return Err("[敏感路径] 拒绝用系统默认程序打开".to_string());
    }
    if !resolved.exists() {
        return Err(format!("路径不存在：{path}"));
    }
    open_with_system(&resolved).map_err(|e| format!("调用系统默认程序打开失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::{read_file_preview, PreviewKind};
    use std::fs;
    use std::path::PathBuf;

    fn temp_file(name: &str, content: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "helm-file-preview-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn text_file_returns_redacted_text() {
        let path = temp_file(
            "notes.txt",
            b"hello\napi_key=sk-HELM_TEST_ABCDEFGH\nworld\n",
        );
        let preview = read_file_preview(path.to_string_lossy().to_string()).unwrap();
        assert_eq!(preview.kind, PreviewKind::Text);
        let content = preview.content.unwrap();
        assert!(content.contains("hello"));
        assert!(content.contains("world"));
        assert!(!content.contains("sk-HELM_TEST_ABCDEFGH"));
        assert!(content.contains("[REDACTED]"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn png_is_image_with_mime_and_base64() {
        let png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        let path = temp_file("pic.png", &png);
        let preview = read_file_preview(path.to_string_lossy().to_string()).unwrap();
        assert_eq!(preview.kind, PreviewKind::Image);
        assert_eq!(preview.mime.as_deref(), Some("image/png"));
        assert!(preview.content.is_some());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn binary_file_is_binary() {
        let bytes = vec![0u8, 1, 2, 3, 4, 5, 255];
        let path = temp_file("blob.bin", &bytes);
        let preview = read_file_preview(path.to_string_lossy().to_string()).unwrap();
        assert_eq!(preview.kind, PreviewKind::Binary);
        assert!(preview.content.is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sensitive_path_is_rejected() {
        let err = read_file_preview("C:/Users/test/.ssh/id_rsa".to_string()).unwrap_err();
        assert!(err.contains("敏感"));
    }

    #[test]
    fn missing_file_is_rejected() {
        let err = read_file_preview("C:/definitely/not/exist.txt".to_string()).unwrap_err();
        assert!(!err.is_empty());
    }
}
