use serde::{Deserialize, Serialize};

/// 只包含纯推理预算档位。Codex `ultra` 与 Claude `ultracode` 会改变工作流或启用
/// 多代理，不属于推理强度，故意不进入协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    #[default]
    Auto,
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn parse(value: Option<&str>) -> Result<Option<Self>, String> {
        let Some(value) = value else {
            return Ok(None);
        };
        let effort = match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "none" => Self::None,
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::Xhigh,
            "max" => Self::Max,
            "ultra" | "ultracode" => {
                return Err("该档位会改变工作流或启用多代理，不能作为推理强度使用".to_string())
            }
            _ => return Err(format!("不支持的推理强度：{value}")),
        };
        Ok(Some(effort))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn is_auto(self) -> bool {
        self == Self::Auto
    }

    pub fn is_claude_level(self) -> bool {
        matches!(
            self,
            Self::Auto | Self::Low | Self::Medium | Self::High | Self::Xhigh | Self::Max
        )
    }
}

pub fn claude_cli_effort_args(effort: ReasoningEffort) -> Vec<&'static str> {
    if effort.is_auto() {
        Vec::new()
    } else {
        vec!["--effort", effort.as_str()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffortSupport {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningEffortSource {
    EngineProbe,
    BuiltinCatalog,
    ProviderDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortCapability {
    pub support: ReasoningEffortSupport,
    pub options: Vec<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<ReasoningEffort>,
    pub source: ReasoningEffortSource,
}

impl ReasoningEffortCapability {
    pub fn unknown(source: ReasoningEffortSource) -> Self {
        Self {
            support: ReasoningEffortSupport::Unknown,
            options: vec![ReasoningEffort::Auto],
            default_effort: None,
            source,
        }
    }
}

fn safe_effort(value: &str) -> Option<ReasoningEffort> {
    ReasoningEffort::parse(Some(value)).ok().flatten()
}

fn claude_model_supports_effort(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('.', "-");
    matches!(normalized.as_str(), "opus" | "sonnet" | "fable")
        || [
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "claude-fable-5",
        ]
        .iter()
        .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix}-")))
}

pub fn claude_reasoning_capability(model: &str, help: &str) -> ReasoningEffortCapability {
    if !claude_model_supports_effort(model) {
        return ReasoningEffortCapability::unknown(ReasoningEffortSource::BuiltinCatalog);
    }
    let help = help.to_ascii_lowercase();
    if !help.contains("--effort") {
        return ReasoningEffortCapability {
            support: ReasoningEffortSupport::Unsupported,
            options: vec![ReasoningEffort::Auto],
            default_effort: None,
            source: ReasoningEffortSource::EngineProbe,
        };
    }
    let mut options = vec![ReasoningEffort::Auto];
    for candidate in ["low", "medium", "high", "xhigh", "max"] {
        if help.contains(candidate) {
            options.push(safe_effort(candidate).expect("static effort is valid"));
        }
    }
    ReasoningEffortCapability {
        support: if options.len() > 1 {
            ReasoningEffortSupport::Supported
        } else {
            ReasoningEffortSupport::Unknown
        },
        options,
        default_effort: None,
        source: ReasoningEffortSource::EngineProbe,
    }
}

pub fn codex_reasoning_capability(
    model: &str,
    response: &serde_json::Value,
) -> ReasoningEffortCapability {
    let Some(models) = response.get("data").and_then(serde_json::Value::as_array) else {
        return ReasoningEffortCapability::unknown(ReasoningEffortSource::EngineProbe);
    };
    let Some(found) = models.iter().find(|entry| {
        entry.get("id").and_then(serde_json::Value::as_str) == Some(model)
            || entry.get("model").and_then(serde_json::Value::as_str) == Some(model)
    }) else {
        return ReasoningEffortCapability::unknown(ReasoningEffortSource::EngineProbe);
    };
    let mut options = vec![ReasoningEffort::Auto];
    if let Some(values) = found
        .get("supportedReasoningEfforts")
        .and_then(serde_json::Value::as_array)
    {
        for value in values {
            let raw = value
                .get("reasoningEffort")
                .or_else(|| value.get("reasoning_effort"))
                .and_then(serde_json::Value::as_str);
            if let Some(effort) = raw.and_then(safe_effort) {
                if !options.contains(&effort) && !effort.is_auto() {
                    options.push(effort);
                }
            }
        }
    }
    let default_effort = found
        .get("defaultReasoningEffort")
        .or_else(|| found.get("default_reasoning_effort"))
        .and_then(serde_json::Value::as_str)
        .and_then(safe_effort)
        .filter(|effort| !effort.is_auto());
    ReasoningEffortCapability {
        support: if options.len() > 1 {
            ReasoningEffortSupport::Supported
        } else {
            ReasoningEffortSupport::Unsupported
        },
        options,
        default_effort,
        source: ReasoningEffortSource::EngineProbe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_workflow_changing_levels() {
        assert!(ReasoningEffort::parse(Some("ultra")).is_err());
        assert!(ReasoningEffort::parse(Some("ultracode")).is_err());
        assert_eq!(
            ReasoningEffort::parse(Some("xhigh")).unwrap(),
            Some(ReasoningEffort::Xhigh)
        );
    }

    #[test]
    fn claude_auto_resets_with_environment_only_and_explicit_levels_use_flag() {
        assert!(claude_cli_effort_args(ReasoningEffort::Auto).is_empty());
        assert_eq!(
            claude_cli_effort_args(ReasoningEffort::High),
            vec!["--effort", "high"]
        );
    }

    #[test]
    fn claude_probe_intersects_known_model_and_cli_levels() {
        let capability = claude_reasoning_capability(
            "claude-sonnet-4.6",
            "--effort <level> (low, medium, high, xhigh, max)",
        );
        assert_eq!(capability.support, ReasoningEffortSupport::Supported);
        assert_eq!(
            capability.options,
            vec![
                ReasoningEffort::Auto,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::Xhigh,
                ReasoningEffort::Max,
            ]
        );
        assert_eq!(
            claude_reasoning_capability("third-party-model", "--effort low, high").support,
            ReasoningEffortSupport::Unknown
        );
    }

    #[test]
    fn codex_probe_uses_exact_model_and_drops_ultra() {
        let response = serde_json::json!({
            "data": [{
                "id": "gpt-5-codex",
                "model": "gpt-5-codex",
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort":"low","description":""},
                    {"reasoningEffort":"high","description":""},
                    {"reasoningEffort":"ultra","description":"workflow mode"}
                ]
            }]
        });
        let capability = codex_reasoning_capability("gpt-5-codex", &response);
        assert_eq!(capability.support, ReasoningEffortSupport::Supported);
        assert_eq!(
            capability.options,
            vec![
                ReasoningEffort::Auto,
                ReasoningEffort::Low,
                ReasoningEffort::High
            ]
        );
        assert_eq!(capability.default_effort, Some(ReasoningEffort::Medium));
    }
}
