use crate::permissions::EngineCapabilityManifest;

pub fn parse_claude_version(output: &str) -> Option<(u64, u64, u64)> {
    output.split_whitespace().find_map(|token| {
        let token =
            token.trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
        let mut parts = token.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        (parts.next().is_none()).then_some((major, minor, patch))
    })
}

pub fn claude_capability_manifest(
    version_output: &str,
    contract_probe_verified: bool,
) -> EngineCapabilityManifest {
    let parsed = parse_claude_version(version_output);
    // RuntimeManaged compatibility is established by the live hook/defer exchange, not a
    // version allowlist.
    let verified = contract_probe_verified;
    EngineCapabilityManifest {
        engine: "claude-code".to_string(),
        version: parsed
            .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
            .unwrap_or_else(|| "unknown".to_string()),
        supports_defer: verified,
        // 并行 ToolCall 审批必须由单独的批量 contract probe 证明，不能从 defer 推断。
        supports_parallel_tool_approval: false,
        supports_native_sandbox: false,
        verified,
    }
}

pub fn defer_contract_probe_succeeded(output: &str, expected_command: &str) -> bool {
    let mut deferred_tool_id = None;
    let mut executed_tool_ids = std::collections::HashSet::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if event.get("type").and_then(serde_json::Value::as_str) == Some("result")
            && event.get("stop_reason").and_then(serde_json::Value::as_str) == Some("tool_deferred")
        {
            let deferred = &event["deferred_tool_use"];
            if deferred.get("name").and_then(serde_json::Value::as_str) == Some("Bash")
                && deferred
                    .pointer("/input/command")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected_command)
            {
                deferred_tool_id = deferred
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string);
            }
        }
        if let Some(content) = event
            .pointer("/message/content")
            .and_then(|value| value.as_array())
        {
            for item in content {
                if item.get("type").and_then(serde_json::Value::as_str) == Some("tool_result") {
                    if let Some(id) = item.get("tool_use_id").and_then(serde_json::Value::as_str) {
                        executed_tool_ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    deferred_tool_id.is_some_and(|id| !executed_tool_ids.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::{claude_capability_manifest, defer_contract_probe_succeeded, parse_claude_version};

    #[test]
    fn parses_the_real_claude_version_shape() {
        assert_eq!(
            parse_claude_version("2.1.207 (Claude Code)"),
            Some((2, 1, 207))
        );
        assert_eq!(parse_claude_version("claude version unknown"), None);
    }

    #[test]
    fn version_strings_never_grant_capabilities_without_a_contract_probe() {
        for version in ["2.1.207", "2.1.217", "2.2.0", "99.0.0"] {
            let unprobed = claude_capability_manifest(version, false);
            assert!(!unprobed.verified, "{version}");
            assert!(!unprobed.supports_defer, "{version}");

            let probed = claude_capability_manifest(version, true);
            assert!(probed.verified, "{version}");
            assert!(probed.supports_defer, "{version}");
        }

        let non_semver = claude_capability_manifest("vendor-build", true);
        assert_eq!(non_semver.version, "unknown");
        assert!(non_semver.verified);
        assert!(non_semver.supports_defer);
    }

    #[test]
    fn defer_probe_requires_exact_deferred_tool_and_no_execution_result() {
        let success = concat!(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo HELM_PROBE\"}}]}}\n",
            "{\"type\":\"result\",\"stop_reason\":\"tool_deferred\",\"deferred_tool_use\":{\"id\":\"tool-1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo HELM_PROBE\"}}}\n"
        );
        assert!(defer_contract_probe_succeeded(success, "echo HELM_PROBE"));

        let executed = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"tool-1\",\"content\":\"HELM_PROBE\"}]}}\n",
            "{\"type\":\"result\",\"stop_reason\":\"tool_deferred\",\"deferred_tool_use\":{\"id\":\"tool-1\",\"name\":\"Bash\",\"input\":{\"command\":\"echo HELM_PROBE\"}}}\n"
        );
        assert!(!defer_contract_probe_succeeded(executed, "echo HELM_PROBE"));
        assert!(!defer_contract_probe_succeeded(
            success,
            "echo OTHER_COMMAND"
        ));
    }
}
