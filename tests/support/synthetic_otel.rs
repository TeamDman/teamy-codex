//! Invented release-shaped fixture; contains no real account or session data.
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value::Value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use prost::Message;

fn text(key: &str, value: &str) -> KeyValue {
    kv(key, Value::StringValue(value.into()))
}
fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
    }
}
fn base(name: &str) -> Vec<KeyValue> {
    vec![
        text("event.name", name),
        text("conversation.id", "synthetic-session"),
        text("call_id", "synthetic-call"),
        text("tool_name", "exec_command"),
    ]
}

pub fn payload() -> Vec<u8> {
    let mut result = base("codex.tool_result");
    result.extend([
        text(
            "arguments",
            r#"{"cmd":"Write-Output 'synthetic'","shell":"powershell"}"#,
        ),
        text("output", "synthetic"),
        text("duration_ms", "42"),
        text("success", "true"),
        kv("output_truncated", Value::BoolValue(false)),
    ]);
    let mut decision = base("codex.tool_decision");
    decision.push(text("decision", "approved"));
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![
                    LogRecord {
                        event_name: "event codex-rs/otel/src/events/shared.rs:123".into(),
                        attributes: result,
                        ..Default::default()
                    },
                    LogRecord {
                        event_name: "event codex-rs/otel/src/events/shared.rs:123".into(),
                        attributes: decision,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}
