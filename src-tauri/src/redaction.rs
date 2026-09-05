use crate::protocol::AgentEvent;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

const REDACTED: &str = "[REDACTED]";

pub fn sanitize_agent_event(event: &AgentEvent) -> AgentEvent {
    let Ok(mut value) = serde_json::to_value(event) else {
        return event.clone();
    };
    redact_value(&mut value);
    serde_json::from_value(value).unwrap_or_else(|_| event.clone())
}

pub fn redact_text(input: &str) -> String {
    let mut output = input.to_string();
    if let Ok(mut value) = serde_json::from_str::<Value>(input) {
        let changed = redact_value(&mut value);
        if changed {
            return serde_json::to_string_pretty(&value).unwrap_or(output);
        }
    }
    for regex in secret_patterns() {
        output = regex.replace_all(&output, REDACTED).into_owned();
    }
    output
}

fn redact_value(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            for (key, value) in object {
                if is_sensitive_key(key) {
                    if !value.is_null() {
                        *value = Value::String(REDACTED.to_string());
                        changed = true;
                    }
                } else {
                    changed |= redact_value(value);
                }
            }
            changed
        }
        Value::Array(values) => values
            .iter_mut()
            .fold(false, |changed, value| redact_value(value) || changed),
        Value::String(text) => {
            let redacted = redact_text_without_json(text);
            if redacted == *text {
                false
            } else {
                *text = redacted;
                true
            }
        }
        _ => false,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '.'], "_");
    [
        "api_key",
        "apikey",
        "auth_token",
        "access_token",
        "refresh_token",
        "authorization",
        "password",
        "secret",
        "credential",
    ]
    .iter()
    .any(|part| normalized.contains(part))
}

fn redact_text_without_json(input: &str) -> String {
    secret_patterns()
        .iter()
        .fold(input.to_string(), |text, regex| {
            regex.replace_all(&text, REDACTED).into_owned()
        })
}

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/=-]{4,}").unwrap(),
            Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}\b").unwrap(),
            Regex::new(
                r#"(?i)[\"']?[a-z0-9_.-]*(?:api[_-]?key|auth[_-]?token|access[_-]?token|refresh[_-]?token|secret|password|authorization|credential)[a-z0-9_.-]*[\"']?\s*[:=]\s*[\"']?[^\"',\s}\r\n]+"#,
            )
            .unwrap(),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::{redact_text, sanitize_agent_event};
    use crate::protocol::{AgentEvent, ToolStatus};

    #[test]
    fn structured_and_inline_credentials_are_redacted_without_hiding_normal_values() {
        let sentinel = "sk-HELM_TEST_SECRET_123456";
        let json = format!(
            r#"{{"env":{{"ANTHROPIC_AUTH_TOKEN":"{sentinel}","MODEL":"safe-model"}},"input_tokens":42}}"#
        );
        let redacted = redact_text(&json);
        assert!(!redacted.contains(sentinel));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.contains("safe-model"));
        assert!(redacted.contains("42"));

        let inline = redact_text(&format!("Authorization: Bearer {sentinel}"));
        assert!(!inline.contains(sentinel));
        assert!(inline.contains("[REDACTED]"));
    }

    #[test]
    fn tool_results_are_sanitized_before_serialization() {
        let sentinel = "sk-HELM_TEST_EVENT_123456";
        let event = AgentEvent::ToolResult {
            session_id: "session-1".to_string(),
            id: "tool-1".to_string(),
            status: ToolStatus::Success,
            output: Some(format!(r#"{{"OPENAI_API_KEY":"{sentinel}"}}"#)),
            diff: None,
            outcome: None,
            started: None,
            has_output: None,
            retryable: None,
            denial_source: None,
            native_denial_code: None,
        };
        let sanitized = sanitize_agent_event(&event);
        let encoded = serde_json::to_string(&sanitized).unwrap();
        assert!(!encoded.contains(sentinel));
        assert!(encoded.contains("REDACTED"));
    }
}
