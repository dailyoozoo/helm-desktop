use crate::permissions::{
    ActionDescriptor, Capability, PermissionDecision, PermissionEffect, PermissionRule,
    PermissionScope,
};

pub fn evaluate_action(
    action: &ActionDescriptor,
    rules: &[PermissionRule],
    now_ms: i64,
    policy_version: u64,
) -> PermissionDecision {
    if action.invalid_reason.is_some() || matches!(action.capability, Capability::Unknown(_)) {
        return PermissionDecision {
            effect: PermissionEffect::Deny,
            reason: "unknown or invalid capability fails closed".to_string(),
            rule_id: None,
            policy_version,
        };
    }

    let matching = rules
        .iter()
        .filter(|rule| rule_matches(action, rule, now_ms))
        .collect::<Vec<_>>();
    // 显式 Deny 是 Helm 的硬上限：更窄作用域的 Allow/Ask 只能收紧或放行未被
    // 禁止的动作，不能把全局/项目级禁令重新打开。
    let selected = matching
        .iter()
        .copied()
        .filter(|rule| rule.effect == PermissionEffect::Deny)
        .max_by_key(|rule| rule_specificity(rule))
        .or_else(|| {
            matching
                .iter()
                .copied()
                .max_by_key(|rule| rule_specificity(rule))
        });

    match selected {
        Some(rule) => PermissionDecision {
            effect: rule.effect,
            reason: format!("matched permission rule {}", rule.id),
            rule_id: Some(rule.id.clone()),
            policy_version,
        },
        None => PermissionDecision {
            effect: PermissionEffect::Ask,
            reason: "no matching permission rule".to_string(),
            rule_id: None,
            policy_version,
        },
    }
}

fn rule_matches(action: &ActionDescriptor, rule: &PermissionRule, now_ms: i64) -> bool {
    let process_allow_requires_matcher = action.capability == Capability::ProcessExec
        && rule.effect == PermissionEffect::Allow
        && rule.scope != PermissionScope::Once;
    let is_session_exec_allow =
        process_allow_requires_matcher && rule.scope == PermissionScope::Session;
    let exact_process_allow = process_allow_requires_matcher
        && !is_session_exec_allow
        && rule
            .resource_pattern
            .as_deref()
            .is_some_and(|pattern| crate::permissions::process_exec_rule_matches(pattern, action));
    let session_process_allow = is_session_exec_allow
        && rule.resource_pattern.as_deref().is_some_and(|pattern| {
            crate::permissions::process_exec_session_rule_matches(pattern, action)
        });
    let process_allow_matched = exact_process_allow || session_process_allow;
    if rule.principal != action.principal
        || rule
            .engine
            .as_deref()
            .is_some_and(|engine| engine != action.engine)
        || rule.capability != action.capability
        || (!process_allow_matched
            && rule
                .operation
                .as_deref()
                .is_some_and(|operation| operation != action.operation))
        || rule
            .expires_at
            .is_some_and(|expires_at| expires_at <= now_ms)
        || rule.max_uses.is_some_and(|max_uses| rule.uses >= max_uses)
        || !scope_matches(action, rule)
    {
        return false;
    }
    if process_allow_requires_matcher {
        return process_allow_matched;
    }
    match rule.resource_pattern.as_deref() {
        Some(_) if action.resources.is_empty() => false,
        Some(pattern) if rule.effect == PermissionEffect::Deny => action
            .resources
            .iter()
            .any(|resource| resource_matches(pattern, resource)),
        Some(pattern) => action
            .resources
            .iter()
            .all(|resource| resource_matches(pattern, resource)),
        None => true,
    }
}

fn scope_matches(action: &ActionDescriptor, rule: &PermissionRule) -> bool {
    match rule.scope {
        PermissionScope::Global => true,
        PermissionScope::Once => {
            rule.scope_binding.tool_call_id.as_deref() == Some(&action.tool_call_id)
                && rule.scope_binding.turn_id.as_deref() == Some(&action.turn_id)
                && rule.scope_binding.session_id.as_deref() == Some(&action.session_id)
        }
        PermissionScope::Turn => {
            rule.scope_binding.turn_id.as_deref() == Some(&action.turn_id)
                && rule.scope_binding.session_id.as_deref() == Some(&action.session_id)
        }
        PermissionScope::Session => {
            rule.scope_binding.session_id.as_deref() == Some(&action.session_id)
        }
        PermissionScope::Project => match (
            rule.scope_binding.project_root.as_deref(),
            action.cwd.as_deref(),
        ) {
            (Some(root), Some(cwd)) => path_within(cwd, root),
            _ => false,
        },
    }
}

fn resource_matches(pattern: &str, resource: &str) -> bool {
    let pattern = normalize_path(pattern);
    let resource = normalize_path(resource);
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return resource == prefix || resource.starts_with(&format!("{prefix}/"));
    }
    pattern == resource
}

fn path_within(path: &str, root: &str) -> bool {
    let path = normalize_path(path);
    let root = normalize_path(root).trim_end_matches('/').to_string();
    path == root || path.starts_with(&format!("{root}/"))
}

fn normalize_path(value: &str) -> String {
    value.replace('\\', "/").to_lowercase()
}

fn scope_specificity(scope: PermissionScope) -> u8 {
    match scope {
        PermissionScope::Global => 1,
        PermissionScope::Project => 2,
        PermissionScope::Session => 3,
        PermissionScope::Turn => 4,
        PermissionScope::Once => 5,
    }
}

fn effect_precedence(effect: PermissionEffect) -> u8 {
    match effect {
        PermissionEffect::Allow => 1,
        PermissionEffect::Ask => 2,
        PermissionEffect::Deny => 3,
    }
}

fn rule_specificity(rule: &PermissionRule) -> (u8, u8, u8, u8) {
    (
        scope_specificity(rule.scope),
        u8::from(rule.resource_pattern.is_some()),
        u8::from(rule.operation.is_some()),
        effect_precedence(rule.effect),
    )
}

#[cfg(test)]
mod tests {
    use super::evaluate_action;
    use crate::permissions::{
        normalize_tool_action_for_principal, Capability, PermissionEffect, PermissionRule,
        PermissionScope, PermissionScopeBinding,
    };
    use serde_json::json;

    fn rule(id: &str, effect: PermissionEffect, scope: PermissionScope) -> PermissionRule {
        PermissionRule {
            id: id.to_string(),
            principal: "main-agent".to_string(),
            effect,
            scope,
            scope_binding: PermissionScopeBinding::default(),
            engine: Some("claude-code".to_string()),
            capability: Capability::ProcessExec,
            operation: Some("ls".to_string()),
            resource_pattern: None,
            created_at: 1,
            expires_at: None,
            max_uses: None,
            uses: 0,
        }
    }

    #[test]
    fn unknown_or_invalid_actions_fail_closed() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "main-agent",
            "CustomTool",
            &json!({}),
            Some("D:/repo"),
        );

        let decision = evaluate_action(&action, &[], 10, 7);

        assert_eq!(decision.effect, PermissionEffect::Deny);
        assert!(decision.reason.contains("unknown"));
        assert_eq!(decision.policy_version, 7);
    }

    #[test]
    fn multi_resource_allow_requires_full_coverage_while_deny_matches_any_resource() {
        let mut action = normalize_tool_action_for_principal(
            "codex",
            "session-1",
            "turn-1",
            "tool-1",
            "main-agent",
            "Write",
            &json!({"path":"D:/repo/allowed.txt"}),
            Some("D:/repo"),
        );
        action.resources = vec![
            "D:/repo/allowed.txt".to_string(),
            "D:/repo/not-authorized.txt".to_string(),
        ];
        let matching_rule = |effect| PermissionRule {
            id: format!("{effect:?}-one-resource"),
            principal: "main-agent".to_string(),
            effect,
            scope: PermissionScope::Global,
            scope_binding: PermissionScopeBinding::default(),
            engine: Some("codex".to_string()),
            capability: Capability::FileWrite,
            operation: Some("Write".to_string()),
            resource_pattern: Some("D:/repo/allowed.txt".to_string()),
            created_at: 1,
            expires_at: None,
            max_uses: None,
            uses: 0,
        };

        let allow = evaluate_action(&action, &[matching_rule(PermissionEffect::Allow)], 10, 1);
        assert_eq!(allow.effect, PermissionEffect::Ask);

        let deny = evaluate_action(&action, &[matching_rule(PermissionEffect::Deny)], 10, 1);
        assert_eq!(deny.effect, PermissionEffect::Deny);
    }

    #[test]
    fn explicit_global_deny_cannot_be_weakened_by_a_more_specific_allow() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "main-agent",
            "Bash",
            &json!({"command": "ls -la"}),
            Some("D:/repo/subdir"),
        );
        let global_deny = rule(
            "global-deny",
            PermissionEffect::Deny,
            PermissionScope::Global,
        );
        let mut project_allow = rule(
            "project-allow",
            PermissionEffect::Allow,
            PermissionScope::Project,
        );
        project_allow.scope_binding.project_root = Some("D:/repo".to_string());

        let decision = evaluate_action(&action, &[global_deny, project_allow], 10, 7);

        assert_eq!(decision.effect, PermissionEffect::Deny);
        assert_eq!(decision.rule_id.as_deref(), Some("global-deny"));
    }

    #[test]
    fn equal_specificity_prefers_deny_and_never_crosses_principal_or_engine() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "subagent:reviewer",
            "Bash",
            &json!({"command": "ls"}),
            Some("D:/repo"),
        );
        let mut wrong_principal = rule(
            "main-allow",
            PermissionEffect::Allow,
            PermissionScope::Global,
        );
        wrong_principal.principal = "main-agent".to_string();
        let mut allow = rule(
            "sub-allow",
            PermissionEffect::Allow,
            PermissionScope::Session,
        );
        allow.principal = "subagent:reviewer".to_string();
        allow.scope_binding.session_id = Some("session-1".to_string());
        let mut deny = allow.clone();
        deny.id = "sub-deny".to_string();
        deny.effect = PermissionEffect::Deny;

        let decision = evaluate_action(&action, &[wrong_principal, allow, deny], 10, 7);

        assert_eq!(decision.effect, PermissionEffect::Deny);
        assert_eq!(decision.rule_id.as_deref(), Some("sub-deny"));
    }

    #[test]
    fn valid_unmatched_action_requires_approval_instead_of_auto_allowing() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "main-agent",
            "Bash",
            &json!({"command": "npm test"}),
            Some("D:/repo"),
        );

        let decision = evaluate_action(&action, &[], 10, 7);

        assert_eq!(decision.effect, PermissionEffect::Ask);
        assert!(decision.rule_id.is_none());
    }

    #[test]
    fn turn_scope_requires_both_session_and_turn_identity() {
        let action = normalize_tool_action_for_principal(
            "claude-code",
            "session-1",
            "turn-1",
            "tool-1",
            "main-agent",
            "Bash",
            &json!({"command":"git status"}),
            Some("D:/repo"),
        );
        let turn_rule = crate::permissions::build_turn_rule_from_action(&action, 1);

        assert_eq!(
            evaluate_action(&action, std::slice::from_ref(&turn_rule), 1, 1).effect,
            PermissionEffect::Allow
        );
        let mut other_session = action;
        other_session.session_id = "session-2".to_string();
        assert_eq!(
            evaluate_action(&other_session, &[turn_rule], 1, 1).effect,
            PermissionEffect::Ask,
            "turn-1 in another Session must not inherit this Turn Allow"
        );
    }
}
