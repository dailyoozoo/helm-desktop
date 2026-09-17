use crate::output_limits::{
    bounded_json_size, bounded_progress, bounded_text, MAX_TOOL_OUTPUT_BYTES,
};
use crate::protocol::{AgentEvent, PlanStep, Role, TurnPresentation, TurnPresentationContent};
use std::collections::HashMap;

const MAX_PRESENTATION_BYTES: usize = 65_536;
const MAX_PLAN_STEPS: usize = 256;

#[derive(Debug, Clone)]
struct PendingText {
    text: String,
    event_seq: u64,
    ts: i64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TurnPresentationBuffer {
    assistant: Option<PendingText>,
    user: Option<PendingText>,
    thinking: Option<PendingText>,
    tool_output: HashMap<String, String>,
}

impl TurnPresentationBuffer {
    pub fn records_for_event(
        &self,
        turn_id: &str,
        event_seq: u64,
        ts: i64,
        event: &AgentEvent,
        terminal: bool,
    ) -> Vec<TurnPresentation> {
        let mut records = Vec::new();
        match event {
            AgentEvent::ThinkingComplete { text, .. } => {
                records.push(TurnPresentation {
                    turn_id: turn_id.to_string(),
                    event_seq: self
                        .thinking
                        .as_ref()
                        .map_or(event_seq, |pending| pending.event_seq),
                    ts: self.thinking.as_ref().map_or(ts, |pending| pending.ts),
                    ended_at: Some(ts),
                    reverted: false,
                    content: TurnPresentationContent::Thinking {
                        text: bounded_text(text, MAX_PRESENTATION_BYTES),
                        complete: true,
                    },
                });
            }
            AgentEvent::PlanUpdate { steps, .. } => {
                let mut stored = Vec::new();
                let mut remaining = MAX_PRESENTATION_BYTES.saturating_sub(1024);
                let mut truncated = steps.len() > MAX_PLAN_STEPS;
                for step in steps.iter().take(MAX_PLAN_STEPS) {
                    let text = bounded_text(&step.text, 4096);
                    truncated |= text != step.text;
                    let bounded = PlanStep {
                        text,
                        status: step.status,
                    };
                    let Some(size) = bounded_json_size(&bounded, remaining) else {
                        truncated = true;
                        break;
                    };
                    remaining = remaining.saturating_sub(size + 1);
                    stored.push(bounded);
                }
                records.push(TurnPresentation {
                    turn_id: turn_id.to_string(),
                    event_seq,
                    ts,
                    ended_at: None,
                    reverted: false,
                    content: TurnPresentationContent::Plan {
                        steps: stored,
                        truncated,
                    },
                });
            }
            _ => {}
        }
        if terminal {
            for (role, pending) in [(Role::Assistant, &self.assistant), (Role::User, &self.user)] {
                if let Some(pending) = pending {
                    records.push(TurnPresentation {
                        turn_id: turn_id.to_string(),
                        event_seq: pending.event_seq,
                        ts: pending.ts,
                        ended_at: Some(ts),
                        reverted: false,
                        content: TurnPresentationContent::Message {
                            role,
                            text: pending.text.clone(),
                            complete: false,
                        },
                    });
                }
            }
            if let Some(pending) = &self.thinking {
                records.push(TurnPresentation {
                    turn_id: turn_id.to_string(),
                    event_seq: pending.event_seq,
                    ts: pending.ts,
                    ended_at: Some(ts),
                    reverted: false,
                    content: TurnPresentationContent::Thinking {
                        text: pending.text.clone(),
                        complete: false,
                    },
                });
            }
        }
        records
    }

    pub fn tool_output(&self, id: &str) -> Option<&str> {
        self.tool_output.get(id).map(String::as_str)
    }

    pub fn pending_tool_outputs(&self) -> Vec<(String, String)> {
        self.tool_output
            .iter()
            .map(|(id, output)| (id.clone(), output.clone()))
            .collect()
    }

    pub fn apply(&mut self, event: &AgentEvent, event_seq: u64, ts: i64, terminal: bool) {
        match event {
            AgentEvent::MessageDelta { role, text, .. } => {
                let pending = match role {
                    Role::Assistant => &mut self.assistant,
                    Role::User => &mut self.user,
                };
                append_pending(pending, text, event_seq, ts);
            }
            AgentEvent::MessageComplete { role, .. } => match role {
                Role::Assistant => self.assistant = None,
                Role::User => self.user = None,
            },
            AgentEvent::ThinkingDelta { text, .. } => {
                append_pending(&mut self.thinking, text, event_seq, ts);
            }
            AgentEvent::ThinkingComplete { .. } => self.thinking = None,
            AgentEvent::ToolProgress { id, chunk, .. } => {
                let output = self.tool_output.entry(id.clone()).or_default();
                let bounded = bounded_progress(chunk, output.len() as u64);
                output.push_str(&bounded);
            }
            AgentEvent::ToolResult { id, .. } => {
                self.tool_output.remove(id);
            }
            _ => {}
        }
        if terminal {
            *self = Self::default();
        }
    }
}

fn append_pending(pending: &mut Option<PendingText>, text: &str, event_seq: u64, ts: i64) {
    if text.is_empty() {
        return;
    }
    let pending = pending.get_or_insert_with(|| PendingText {
        text: String::new(),
        event_seq,
        ts,
    });
    let chunk = bounded_progress(text, pending.text.len() as u64);
    pending.text.push_str(&chunk);
    debug_assert!(pending.text.len() <= MAX_TOOL_OUTPUT_BYTES);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_preserves_only_incomplete_visible_content() {
        let mut buffer = TurnPresentationBuffer::default();
        let message = AgentEvent::MessageDelta {
            session_id: "native".into(),
            role: Role::Assistant,
            text: "partial".into(),
        };
        buffer.apply(&message, 2, 10, false);
        let thinking = AgentEvent::ThinkingDelta {
            session_id: "native".into(),
            text: "visible".into(),
        };
        buffer.apply(&thinking, 3, 11, false);
        let records = buffer.records_for_event("turn", 4, 12, &message, true);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].event_seq, 2);
        assert_eq!(records[0].ts, 10);
        assert_eq!(records[0].ended_at, Some(12));
        assert!(
            matches!(&records[0].content, TurnPresentationContent::Message { text, complete: false, .. } if text == "partial")
        );
        buffer.apply(
            &AgentEvent::MessageComplete {
                session_id: "native".into(),
                role: Role::Assistant,
                text: "partial".into(),
            },
            4,
            12,
            false,
        );
        assert_eq!(
            buffer
                .records_for_event("turn", 5, 13, &message, true)
                .len(),
            1
        );
    }

    #[test]
    fn pending_text_is_utf8_safe_and_bounded() {
        let mut buffer = TurnPresentationBuffer::default();
        let event = AgentEvent::ThinkingDelta {
            session_id: "native".into(),
            text: "可见摘要".repeat(20_000),
        };
        buffer.apply(&event, 1, 10, false);
        buffer.apply(&event, 2, 11, false);
        let records = buffer.records_for_event("turn", 3, 12, &event, true);
        let TurnPresentationContent::Thinking { text, complete } = &records[0].content else {
            panic!("thinking record");
        };
        assert!(!complete);
        assert!(text.len() <= MAX_PRESENTATION_BYTES);
        assert!(text.ends_with(crate::output_limits::OUTPUT_TRUNCATED));
    }
}
