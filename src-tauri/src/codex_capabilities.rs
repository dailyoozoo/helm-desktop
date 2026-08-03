use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCapabilityManifest {
    pub version: String,
    pub supports_app_server_v2: bool,
    pub supports_command_approval: bool,
    pub supports_file_approval: bool,
    pub supports_permission_profile_approval: bool,
    pub supports_native_sandbox: bool,
    pub verified: bool,
}

pub fn parse_codex_version(output: &str) -> Option<(u64, u64, u64)> {
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

pub fn codex_capability_manifest(
    version_output: &str,
    contract_probe_verified: bool,
) -> CodexCapabilityManifest {
    // Static probe data is diagnostic only. Runtime behavior is decided by the
    // live app-server handshake and the capability snapshot for that generation.
    let parsed = parse_codex_version(version_output);
    let verified = contract_probe_verified;
    CodexCapabilityManifest {
        version: parsed
            .map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
            .unwrap_or_else(|| "unknown".to_string()),
        supports_app_server_v2: verified,
        supports_command_approval: verified,
        supports_file_approval: verified,
        supports_permission_profile_approval: verified,
        supports_native_sandbox: verified,
        verified,
    }
}

#[cfg(test)]
mod tests {
    use super::{codex_capability_manifest, parse_codex_version};

    #[test]
    fn version_strings_never_replace_the_contract_probe() {
        assert_eq!(parse_codex_version("codex-cli 0.144.1"), Some((0, 144, 1)));
        assert_eq!(parse_codex_version("codex unknown"), None);

        let installed = codex_capability_manifest("codex-cli 0.144.1", false);
        assert!(!installed.verified);
        assert!(!installed.supports_app_server_v2);
        assert!(!installed.supports_command_approval);
        assert!(!installed.supports_file_approval);
        assert!(!installed.supports_permission_profile_approval);
        assert!(!installed.supports_native_sandbox);

        let future = codex_capability_manifest("codex-cli 0.145.0", false);
        assert!(!future.verified);
        assert!(!future.supports_command_approval);

        for version in ["0.144.1", "0.145.0", "1.0.0", "99.0.0"] {
            let probed = codex_capability_manifest(version, true);
            assert!(probed.verified, "{version}");
            assert!(probed.supports_app_server_v2, "{version}");
        }

        let non_semver = codex_capability_manifest("vendor-build", true);
        assert_eq!(non_semver.version, "unknown");
        assert!(non_semver.verified);
        assert!(non_semver.supports_app_server_v2);
    }
}
