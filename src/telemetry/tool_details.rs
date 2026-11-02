use super::{CodexEvent, CommandDetail, FieldValue, command_detail};

/// Known decision fields; the original event still holds all other fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDecision {
    pub tool_name: Option<String>,
    pub decision: Option<String>,
    pub decision_source: Option<String>,
}

/// Known result fields; missing and invalid values remain unknown.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub tool_name: Option<String>,
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
    pub detail: CommandDetail,
}

fn text(event: &CodexEvent, key: &str) -> Option<String> {
    match event.field(key) {
        Some(FieldValue::Text(value)) => Some(value.clone()),
        _ => None,
    }
}

/// Decode a decision only for the matching event name.
#[must_use]
pub fn tool_decision(event: &CodexEvent) -> Option<ToolDecision> {
    (event.name == "codex.tool_decision").then(|| ToolDecision {
        tool_name: text(event, "tool_name"),
        decision: text(event, "decision"),
        decision_source: text(event, "source"),
    })
}

/// Decode release-style string values as well as typed OTLP values.
#[must_use]
pub fn tool_result(event: &CodexEvent) -> Option<ToolResult> {
    if event.name != "codex.tool_result" {
        return None;
    }
    let success = match event.field("success") {
        Some(FieldValue::Bool(value)) => Some(*value),
        Some(FieldValue::Text(value)) if value == "true" => Some(true),
        Some(FieldValue::Text(value)) if value == "false" => Some(false),
        _ => None,
    };
    let duration_ms = match event.field("duration_ms") {
        Some(FieldValue::Integer(value)) => u64::try_from(*value).ok(),
        Some(FieldValue::Text(value)) => value.parse().ok(),
        _ => None,
    };
    Some(ToolResult {
        tool_name: text(event, "tool_name"),
        success,
        duration_ms,
        detail: command_detail(event),
    })
}
