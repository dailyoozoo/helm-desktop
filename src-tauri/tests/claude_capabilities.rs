use helm_lib::claude_capabilities::{claude_capability_manifest, parse_claude_version};

#[test]
fn parses_versions_but_requires_the_live_contract_probe_for_capabilities() {
    assert_eq!(
        parse_claude_version("2.1.207 (Claude Code)"),
        Some((2, 1, 207))
    );
    assert_eq!(parse_claude_version("claude version unknown"), None);

    for version in ["2.1.207 (Claude Code)", "2.2.0 (Claude Code)"] {
        let unprobed = claude_capability_manifest(version, false);
        assert!(!unprobed.verified && !unprobed.supports_defer, "{version}");

        let probed = claude_capability_manifest(version, true);
        assert!(probed.verified && probed.supports_defer, "{version}");
    }

    let vendor_build = claude_capability_manifest("vendor-build", true);
    assert_eq!(vendor_build.version, "unknown");
    assert!(vendor_build.verified && vendor_build.supports_defer);
}
