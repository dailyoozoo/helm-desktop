use crate::protocol::Diff;
use serde::Serialize;
use std::io::{self, Write};

pub const MAX_TOOL_OUTPUT_BYTES: usize = 65_536;
pub const OUTPUT_TRUNCATED: &str = "\n[ledger_output_truncated]";
const DIFF_OMITTED: &str = "\n[tool_diff_omitted] 差异过大，请打开文件查看完整内容";

pub fn bounded_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut boundary = limit.saturating_sub(OUTPUT_TRUNCATED.len());
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    let mut output = value[..boundary].to_string();
    if limit >= OUTPUT_TRUNCATED.len() {
        output.push_str(OUTPUT_TRUNCATED);
    }
    output
}

pub fn bounded_progress(chunk: &str, previous_bytes: u64) -> String {
    let capacity = MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.len();
    if previous_bytes > capacity as u64 {
        return String::new();
    }
    let remaining = capacity.saturating_sub(previous_bytes as usize);
    if chunk.len() <= remaining {
        return chunk.to_string();
    }
    let mut boundary = remaining;
    while !chunk.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    format!("{}{OUTPUT_TRUNCATED}", &chunk[..boundary])
}

struct ByteCounter {
    size: usize,
    limit: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.size = self.size.saturating_add(bytes.len());
        if self.size > self.limit {
            return Err(io::Error::other("output limit"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn bounded_json_size(value: &impl Serialize, limit: usize) -> Option<usize> {
    let mut counter = ByteCounter { size: 0, limit };
    serde_json::to_writer(&mut counter, value)
        .ok()
        .map(|_| counter.size)
}

pub fn bounded_tool_result(
    output: Option<&str>,
    diff: Option<&Diff>,
) -> (Option<String>, Option<Diff>) {
    let diff_size = diff
        .and_then(|diff| bounded_json_size(diff, MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.len()));
    let omitted = diff.is_some() && diff_size.is_none();
    let notice = if omitted { DIFF_OMITTED } else { "" };
    let limit = MAX_TOOL_OUTPUT_BYTES
        .saturating_sub(diff_size.unwrap_or_default())
        .saturating_sub(notice.len());
    let bounded = output.map(|output| bounded_text(output, limit));
    let bounded = if omitted {
        Some(format!("{}{notice}", bounded.unwrap_or_default()))
    } else {
        bounded
    };
    (bounded, diff.filter(|_| !omitted).cloned())
}

pub fn tool_result_bytes(output: Option<&str>, diff: Option<&Diff>) -> u64 {
    let diff_bytes = diff
        .map(|diff| {
            diff.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .fold(diff.path.len() as u64, |total, line| {
                    total.saturating_add(line.text.len() as u64)
                })
        })
        .unwrap_or_default();
    (output.map(str::len).unwrap_or_default() as u64).saturating_add(diff_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DiffHunk, DiffKind, DiffLine};

    #[test]
    fn tool_output_and_diff_share_one_utf8_budget() {
        let diff = Diff {
            path: "file".into(),
            hunks: vec![DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![DiffLine {
                    kind: DiffKind::Add,
                    text: "中文".repeat(100_000),
                }],
            }],
        };
        let (output, diff) = bounded_tool_result(Some(&"中文".repeat(100_000)), Some(&diff));
        assert!(diff.is_none());
        let output = output.unwrap();
        assert!(output.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(output.contains("tool_diff_omitted"));
        assert!(output.contains("ledger_output_truncated"));
    }

    #[test]
    fn progress_has_one_total_budget_and_one_truncation_notice() {
        let first = "a".repeat(MAX_TOOL_OUTPUT_BYTES - OUTPUT_TRUNCATED.len());
        assert_eq!(bounded_progress(&first, 0), first);
        assert_eq!(
            bounded_progress("extra", first.len() as u64),
            OUTPUT_TRUNCATED
        );
        assert_eq!(
            bounded_progress("later", MAX_TOOL_OUTPUT_BYTES as u64 + 1),
            ""
        );
    }
}
