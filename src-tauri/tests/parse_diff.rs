use helm_lib::parse::parse_claude_line;
use helm_lib::protocol::{AgentEvent, Diff, DiffKind};
use serde_json::{json, Value};

fn diff_from_content(content: Value) -> Option<Diff> {
    let events = parse_claude_line(
        &json!({
            "type": "user",
            "session_id": "diff-session",
            "message": {
                "content": [{"type": "tool_result", "tool_use_id": "diff-tool", "content": content}]
            }
        })
        .to_string(),
    );
    assert_eq!(events.len(), 1);
    match events.into_iter().next().unwrap() {
        AgentEvent::ToolResult { diff, .. } => diff,
        event => panic!("Expected a tool result, got {event:?}"),
    }
}

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn lines_text(values: &[String]) -> String {
    if values.is_empty() {
        String::new()
    } else {
        format!("{}\n", values.join("\n"))
    }
}

fn parse_diff(old_lines: &[String], new_lines: &[String]) -> Option<Diff> {
    diff_from_content(json!([{
        "type": "diff",
        "path": "demo.txt",
        "old_string": lines_text(old_lines),
        "new_string": lines_text(new_lines)
    }]))
}

fn expect_rebuild(old_lines: &[String], new_lines: &[String], diff: Option<&Diff>) {
    let mut old_index = 0;
    let mut rebuilt = Vec::new();
    for hunk in diff.into_iter().flat_map(|diff| &diff.hunks) {
        let start = hunk.old_start as usize - 1;
        assert!(start >= old_index);
        rebuilt.extend_from_slice(&old_lines[old_index..start]);
        old_index = start;
        assert_eq!(hunk.new_start as usize, rebuilt.len() + 1);
        for line in &hunk.lines {
            if line.kind != DiffKind::Add {
                assert_eq!(old_lines.get(old_index), Some(&line.text));
                old_index += 1;
            }
            if line.kind != DiffKind::Del {
                rebuilt.push(line.text.clone());
            }
        }
    }
    rebuilt.extend_from_slice(&old_lines[old_index..]);
    assert_eq!(rebuilt, new_lines);
}

fn lcs_length(old_lines: &[String], new_lines: &[String]) -> usize {
    let mut previous = vec![0; new_lines.len() + 1];
    for old_line in old_lines {
        let mut current = vec![0; new_lines.len() + 1];
        for (index, new_line) in new_lines.iter().enumerate() {
            current[index + 1] = if old_line == new_line {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        previous = current;
    }
    previous[new_lines.len()]
}

fn change_count(diff: Option<&Diff>) -> usize {
    diff.into_iter()
        .flat_map(|diff| &diff.hunks)
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind != DiffKind::Ctx)
        .count()
}

struct DeterministicRandom(u32);

impl DeterministicRandom {
    fn next(&mut self, limit: usize) -> usize {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 as usize % limit
    }
}

fn edited_lines(
    original: &[String],
    count: usize,
    random: &mut DeterministicRandom,
) -> Vec<String> {
    let mut result = original.to_vec();
    for _edit_index in 0..count {
        let position = random.next(result.len() + 1);
        let operation = random.next(3);
        let text = format!("修改-{}", random.next(12));
        if operation == 0 || result.is_empty() {
            result.insert(position, text);
        } else if position < result.len() {
            if operation == 1 {
                result.remove(position);
            } else {
                result[position] = text;
            }
        } else if operation == 2 {
            result.push(text);
        }
    }
    result
}

#[test]
fn separated_changes_preserve_middle_context_and_hunk_offsets() {
    let old_lines = lines(&["header", "before", "keep", "after", "footer"]);
    let new_lines = lines(&[
        "header",
        "changed-before",
        "keep",
        "changed-after",
        "footer",
    ]);
    let diff = parse_diff(&old_lines, &new_lines).unwrap();
    assert_eq!(
        serde_json::to_value(&diff).unwrap(),
        json!({
            "path": "demo.txt",
            "hunks": [{
                "oldStart": 2,
                "newStart": 2,
                "lines": [
                    {"kind": "del", "text": "before"},
                    {"kind": "add", "text": "changed-before"},
                    {"kind": "ctx", "text": "keep"},
                    {"kind": "del", "text": "after"},
                    {"kind": "add", "text": "changed-after"}
                ]
            }]
        })
    );
    expect_rebuild(&old_lines, &new_lines, Some(&diff));
}

#[test]
fn small_diff_cases_reconstruct_with_minimal_edits() {
    let cases: &[(&[&str], &[&str])] = &[
        (&[], &[]),
        (&["same"], &["same"]),
        (&[], &["新增", "🙂"]),
        (&["移除", "🙂"], &[]),
        (&["head", "tail"], &["insert", "head", "tail"]),
        (&["head", "tail"], &["head", "tail", "append"]),
        (&["head", "tail"], &["tail"]),
        (&["head", "tail"], &["head"]),
        (&["A", "B", "A", "C"], &["B", "A", "B", "D"]),
        (&[""], &["", ""]),
    ];
    for (old_values, new_values) in cases {
        let old_lines = lines(old_values);
        let new_lines = lines(new_values);
        let diff = parse_diff(&old_lines, &new_lines);
        expect_rebuild(&old_lines, &new_lines, diff.as_ref());
        assert_eq!(
            change_count(diff.as_ref()),
            old_lines.len() + new_lines.len() - 2 * lcs_length(&old_lines, &new_lines)
        );
        if old_lines == new_lines {
            assert!(diff.is_none());
        }
    }
}

#[test]
fn explicit_diff_wins_over_text_and_unrelated_content_blocks() {
    assert!(diff_from_content(json!(
        "--- guessed.txt\n+++ guessed.txt\n@@ -1 +1 @@\n-old\n+new"
    ))
    .is_none());
    let diff = diff_from_content(json!([
        null,
        {"message": "unrelated metadata"},
        {"type": "text", "text": "--- guessed.txt\n+++ guessed.txt"},
        {
            "type": "diff",
            "path": "真实.txt",
            "old_string": "头部\r\n旧🙂\r\n\r\n尾部\r\n",
            "new_string": "头部\r\n新🙂\r\n\r\n尾部\r\n"
        }
    ]))
    .unwrap();
    assert_eq!(diff.path, "真实.txt");
    assert_eq!(diff.hunks[0].old_start, 2);
    expect_rebuild(
        &lines(&["头部", "旧🙂", "", "尾部"]),
        &lines(&["头部", "新🙂", "", "尾部"]),
        Some(&diff),
    );
}

#[test]
fn deterministic_small_edits_reconstruct_and_remain_minimal() {
    let mut random = DeterministicRandom(0x5eed1234);
    for _case_index in 0..128 {
        let old_lines: Vec<String> = (0..random.next(65))
            .map(|_| format!("line-{}", random.next(12)))
            .collect();
        let new_lines = edited_lines(&old_lines, 1 + random.next(24), &mut random);
        let diff = parse_diff(&old_lines, &new_lines);
        expect_rebuild(&old_lines, &new_lines, diff.as_ref());
        assert_eq!(
            change_count(diff.as_ref()),
            old_lines.len() + new_lines.len() - 2 * lcs_length(&old_lines, &new_lines)
        );
    }
}

#[test]
fn matrix_cell_limit_switches_to_bounded_lookahead() {
    for line_count in [511, 512] {
        let common: Vec<String> = (0..line_count - 68)
            .map(|index| format!("common-{index}"))
            .collect();
        let mut old_lines = lines(&["old-head"]);
        old_lines.extend((0..65).map(|index| format!("removed-{index}")));
        old_lines.push("anchor".to_string());
        old_lines.extend_from_slice(&common);
        old_lines.push("old-foot".to_string());
        let mut new_lines = lines(&["new-head", "anchor"]);
        new_lines.extend_from_slice(&common);
        new_lines.extend((0..65).map(|index| format!("inserted-{index}")));
        new_lines.push("new-foot".to_string());
        assert_eq!(old_lines.len(), line_count);
        assert_eq!(new_lines.len(), line_count);
        let diff = parse_diff(&old_lines, &new_lines).unwrap();
        expect_rebuild(&old_lines, &new_lines, Some(&diff));
        assert_eq!(
            diff.hunks[0]
                .lines
                .iter()
                .any(|line| line.kind == DiffKind::Ctx && line.text == "anchor"),
            line_count == 511
        );
    }
}

#[test]
fn deterministic_large_edits_reconstruct_with_stable_fallback() {
    let mut random = DeterministicRandom(0x5eed5678);
    for _case_index in 0..16 {
        let old_lines: Vec<String> = (0..700)
            .map(|_| format!("line-{}", random.next(48)))
            .collect();
        let mut new_lines = edited_lines(&old_lines, 80, &mut random);
        new_lines[0] = "changed-first".to_string();
        *new_lines.last_mut().unwrap() = "changed-last".to_string();
        let diff = parse_diff(&old_lines, &new_lines);
        expect_rebuild(&old_lines, &new_lines, diff.as_ref());
        assert_eq!(parse_diff(&old_lines, &new_lines), diff);
    }
}

#[test]
fn large_diff_lookahead_stops_after_64_lines() {
    for distance in [63, 64, 65] {
        let common: Vec<String> = (0..600).map(|index| format!("common-{index}")).collect();
        let mut old_lines = lines(&["old-head"]);
        old_lines.extend((0..distance).map(|index| format!("removed-{index}")));
        old_lines.push("anchor".to_string());
        old_lines.extend_from_slice(&common);
        old_lines.push("old-foot".to_string());
        let mut new_lines = lines(&["new-head", "anchor"]);
        new_lines.extend_from_slice(&common);
        new_lines.push("new-foot".to_string());
        let diff = parse_diff(&old_lines, &new_lines).unwrap();
        expect_rebuild(&old_lines, &new_lines, Some(&diff));
        assert_eq!(
            diff.hunks[0]
                .lines
                .iter()
                .any(|line| line.kind == DiffKind::Ctx && line.text == "anchor"),
            distance <= 64
        );
    }
}

#[test]
fn sparse_3000_line_diff_preserves_context_and_reconstructs() {
    let old_lines: Vec<String> = (0..3000).map(|index| format!("line-{index}")).collect();
    let mut new_lines: Vec<String> = old_lines
        .iter()
        .enumerate()
        .flat_map(|(index, text)| {
            if index % 89 == 0 {
                Vec::new()
            } else if index % 97 == 0 {
                vec![format!("changed-{index}")]
            } else if index % 101 == 0 {
                vec![format!("inserted-{index}"), text.clone()]
            } else {
                vec![text.clone()]
            }
        })
        .collect();
    new_lines[0] = "changed-first".to_string();
    *new_lines.last_mut().unwrap() = "changed-last".to_string();
    let diff = parse_diff(&old_lines, &new_lines).unwrap();
    expect_rebuild(&old_lines, &new_lines, Some(&diff));
    assert!(
        diff.hunks[0]
            .lines
            .iter()
            .filter(|line| line.kind == DiffKind::Ctx)
            .count()
            > 2800
    );
    assert!(diff.hunks[0].lines.len() <= old_lines.len() + new_lines.len());
}

#[test]
fn disjoint_3000_line_diff_reconstructs_without_quadratic_matrix() {
    let old_lines: Vec<String> = (0..3000).map(|index| format!("old-{index}")).collect();
    let new_lines: Vec<String> = (0..3000).map(|index| format!("new-{index}")).collect();
    let diff = parse_diff(&old_lines, &new_lines).unwrap();
    expect_rebuild(&old_lines, &new_lines, Some(&diff));
    assert_eq!(diff.hunks[0].lines.len(), 6000);
    assert!(diff.hunks[0]
        .lines
        .iter()
        .all(|line| line.kind != DiffKind::Ctx));
}
