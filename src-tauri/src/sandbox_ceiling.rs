use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxCeilingMode {
    ReadOnly,
    WorkspaceBuild,
    IsolatedFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxEvidenceLevel {
    PolicyOnly,
    EngineSandbox,
    OsEnforced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxBackendManifest {
    pub backend: String,
    pub evidence: SandboxEvidenceLevel,
    pub supports_read_only: bool,
    pub supports_workspace_build: bool,
    pub supports_isolated_full: bool,
    pub supports_network_deny: bool,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxCeiling {
    pub mode: SandboxCeilingMode,
    pub writable_roots: Vec<String>,
    pub network_allowed: bool,
    pub backend: String,
    pub evidence: SandboxEvidenceLevel,
}

pub fn derive_sandbox_ceiling(
    mode: SandboxCeilingMode,
    project_root: &str,
    network_allowed: bool,
    backend: &SandboxBackendManifest,
) -> Result<SandboxCeiling, String> {
    if !backend.verified {
        return Err(format!("sandbox backend {} is unverified", backend.backend));
    }
    let project_root = project_root.replace('\\', "/");
    let project_root = project_root.trim_end_matches('/');
    if project_root.is_empty() {
        return Err("sandbox project root is empty".to_string());
    }
    if !network_allowed && !backend.supports_network_deny {
        return Err(format!(
            "sandbox backend {} cannot enforce network deny",
            backend.backend
        ));
    }
    let writable_roots = match mode {
        SandboxCeilingMode::ReadOnly if backend.supports_read_only => Vec::new(),
        SandboxCeilingMode::WorkspaceBuild if backend.supports_workspace_build => {
            vec![project_root.to_string()]
        }
        SandboxCeilingMode::IsolatedFull
            if backend.supports_isolated_full
                && backend.evidence == SandboxEvidenceLevel::OsEnforced =>
        {
            vec![project_root.to_string()]
        }
        SandboxCeilingMode::IsolatedFull => {
            return Err(format!(
                "isolated full sandbox is unverified for backend {}",
                backend.backend
            ))
        }
        _ => {
            return Err(format!(
                "sandbox mode {mode:?} is unsupported by backend {}",
                backend.backend
            ))
        }
    };
    Ok(SandboxCeiling {
        mode,
        writable_roots,
        network_allowed,
        backend: backend.backend.clone(),
        evidence: backend.evidence,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        derive_sandbox_ceiling, SandboxBackendManifest, SandboxCeilingMode, SandboxEvidenceLevel,
    };

    #[test]
    fn derives_the_intersection_of_requested_mode_platform_backend_and_policy() {
        let codex_native = SandboxBackendManifest {
            backend: "codex-native".to_string(),
            evidence: SandboxEvidenceLevel::EngineSandbox,
            supports_read_only: true,
            supports_workspace_build: true,
            supports_isolated_full: false,
            supports_network_deny: true,
            verified: true,
        };
        let read_only = derive_sandbox_ceiling(
            SandboxCeilingMode::ReadOnly,
            "D:/repo",
            false,
            &codex_native,
        )
        .unwrap();
        assert!(read_only.writable_roots.is_empty());
        assert!(!read_only.network_allowed);
        assert_eq!(read_only.evidence, SandboxEvidenceLevel::EngineSandbox);

        let workspace = derive_sandbox_ceiling(
            SandboxCeilingMode::WorkspaceBuild,
            "D:/repo",
            false,
            &codex_native,
        )
        .unwrap();
        assert_eq!(workspace.writable_roots, vec!["D:/repo"]);
        assert!(!workspace.network_allowed);

        assert!(derive_sandbox_ceiling(
            SandboxCeilingMode::IsolatedFull,
            "D:/repo",
            true,
            &codex_native,
        )
        .unwrap_err()
        .contains("unverified"));
    }

    #[test]
    fn invalid_roots_and_unverified_backends_fail_closed() {
        let unverified = SandboxBackendManifest {
            backend: "runtime-sandbox-unverified".to_string(),
            evidence: SandboxEvidenceLevel::PolicyOnly,
            supports_read_only: true,
            supports_workspace_build: true,
            supports_isolated_full: true,
            supports_network_deny: false,
            verified: false,
        };
        assert!(
            derive_sandbox_ceiling(SandboxCeilingMode::WorkspaceBuild, "", false, &unverified,)
                .is_err()
        );
        assert!(derive_sandbox_ceiling(
            SandboxCeilingMode::WorkspaceBuild,
            "D:/repo",
            false,
            &unverified,
        )
        .is_err());
    }
}
