use teamy_codex::telemetry::{
    CodexEvent, Correlation, Field, FieldValue, ShellKind, command_detail, correlate,
    escape_display,
};

fn event(
    name: &str,
    source: Option<&str>,
    session: Option<&str>,
    call: Option<&str>,
) -> CodexEvent {
    CodexEvent {
        name: name.into(),
        source: source.map(str::to_owned),
        session: session.map(str::to_owned),
        call_id: call.map(str::to_owned),
        fields: Vec::new(),
    }
}

#[test]
fn pairs_out_of_order_with_explicit_confidence_and_keeps_unmatched() {
    let events = vec![
        event(
            "codex.tool_result",
            Some("source"),
            Some("session"),
            Some("call"),
        ),
        event(
            "codex.tool_decision",
            Some("source"),
            Some("session"),
            Some("call"),
        ),
        event("codex.tool_result", None, Some("session-2"), Some("call")),
        event("codex.tool_decision", None, Some("session-2"), Some("call")),
        event("codex.tool_result", None, Some("session-3"), None),
        event(
            "codex.tool_decision",
            Some("other-source"),
            Some("session"),
            Some("call"),
        ),
        event("unknown-event", None, None, None),
    ];
    let rows = correlate(&events);
    assert_eq!(rows.len(), 5);
    assert_eq!(
        (rows[0].decision, rows[0].result, rows[0].correlation),
        (Some(1), Some(0), Correlation::Exact)
    );
    assert_eq!(
        (rows[1].decision, rows[1].result, rows[1].correlation),
        (Some(3), Some(2), Correlation::SourceUnknown)
    );
    assert_eq!(rows[2].correlation, Correlation::MissingIdentity);
    assert_eq!(rows[3].correlation, Correlation::Unmatched);
    assert_eq!(rows[4].correlation, Correlation::OtherEvent);
}

#[test]
fn duplicates_are_ambiguous_not_arbitrarily_paired() {
    let events = vec![
        event(
            "codex.tool_decision",
            Some("source"),
            Some("session"),
            Some("call"),
        ),
        event(
            "codex.tool_decision",
            Some("source"),
            Some("session"),
            Some("call"),
        ),
        event(
            "codex.tool_result",
            Some("source"),
            Some("session"),
            Some("call"),
        ),
    ];
    let rows = correlate(&events);
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|row| row.correlation == Correlation::Ambiguous)
    );
    assert_eq!(rows.iter().filter(|row| row.decision.is_some()).count(), 2);
    assert_eq!(rows.iter().filter(|row| row.result.is_some()).count(), 1);
}

#[test]
fn typed_unknown_fields_survive_facet_round_trip() {
    let mut record = event(
        "codex.tool_result",
        Some("producer"),
        Some("session"),
        Some("call"),
    );
    record.fields.push(Field {
        key: "new_attribute".into(),
        value: FieldValue::Map(vec![Field {
            key: "values".into(),
            value: FieldValue::Array(vec![
                FieldValue::Integer(42),
                FieldValue::Bool(true),
                FieldValue::Text("42".into()),
            ]),
        }]),
    });
    let json = facet_json::to_string(&record).unwrap();
    let decoded: CodexEvent = facet_json::from_str(&json).unwrap();
    assert_eq!(decoded, record);
}

#[test]
fn result_supports_release_string_fields_without_guessing_invalid_values() {
    use teamy_codex::telemetry::{tool_decision, tool_result};
    let mut result = event("codex.tool_result", None, Some("s"), Some("c"));
    result.fields = vec![
        Field {
            key: "success".into(),
            value: FieldValue::Text("false".into()),
        },
        Field {
            key: "duration_ms".into(),
            value: FieldValue::Text("123".into()),
        },
    ];
    let details = tool_result(&result).unwrap();
    assert_eq!(
        (details.success, details.duration_ms),
        (Some(false), Some(123))
    );
    assert!(tool_decision(&result).is_none());
    result.fields[0].value = FieldValue::Text("not a boolean".into());
    result.fields[1].value = FieldValue::Integer(-1);
    let details = tool_result(&result).unwrap();
    assert_eq!((details.success, details.duration_ms), (None, None));
    let mut decision = event("codex.tool_decision", None, Some("s"), Some("c"));
    decision.fields.push(Field {
        key: "decision".into(),
        value: FieldValue::Text("approved".into()),
    });
    assert_eq!(
        tool_decision(&decision).unwrap().decision.as_deref(),
        Some("approved")
    );
    assert!(tool_result(&decision).is_none());
}

#[test]
fn powershell_arguments_truncation_unknown_fields_and_escaped_display() {
    let mut record = event("codex.tool_result", None, Some("session"), Some("call"));
    record.fields = vec![
        Field { key: "tool_name".into(), value: FieldValue::Text("exec_command".into()) },
        Field { key: "arguments".into(), value: FieldValue::Text(r#"{"cmd":"Get-Date\nWrite-Output 'hello'","shell":"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe","yield_time_ms":1000}"#.into()) },
        Field { key: "output".into(), value: FieldValue::Text("line one\n[... telemetry preview truncated ...]".into()) },
        Field { key: "output_truncated".into(), value: FieldValue::Bool(true) },
        Field { key: "unknown".into(), value: FieldValue::Integer(42) },
    ];
    let detail = command_detail(&record);
    assert!(matches!(detail.shell, ShellKind::PowerShell(_)));
    assert_eq!(
        detail.command.as_deref(),
        Some("Get-Date\nWrite-Output 'hello'")
    );
    assert_eq!(detail.output_truncated, Some(true));
    assert_eq!(record.field("unknown"), Some(&FieldValue::Integer(42)));
    assert!(detail.raw_arguments.unwrap().contains("yield_time_ms"));
    assert_eq!(
        escape_display("a\u{1b}b\u{202e}\u{200f}\nc"),
        "a\\u{1b}b\\u{202e}\\u{200f}\nc"
    );
}

#[test]
fn omitted_shell_and_missing_or_malformed_arguments_remain_unknown() {
    let mut record = event("codex.tool_result", None, None, None);
    record.fields.push(Field {
        key: "arguments".into(),
        value: FieldValue::Text(r#"{"cmd":"Get-Date"}"#.into()),
    });
    record.fields.push(Field {
        key: "tool_name".into(),
        value: FieldValue::Text("exec_command".into()),
    });
    let detail = command_detail(&record);
    assert_eq!(detail.shell, ShellKind::Unknown);
    assert_eq!(detail.command.as_deref(), Some("Get-Date"));
    assert_eq!(detail.output_truncated, None);
    record.fields[1].value = FieldValue::Text("another_tool".into());
    let other_tool = command_detail(&record);
    assert_eq!(other_tool.command, None);
    assert_eq!(other_tool.shell, ShellKind::Unknown);
    record.fields[1].value = FieldValue::Text("exec_command".into());
    record.fields[0].value = FieldValue::Text("not json".into());
    let detail = command_detail(&record);
    assert_eq!(detail.shell, ShellKind::Unknown);
    assert_eq!(detail.command, None);
    assert_eq!(detail.raw_arguments.as_deref(), Some("not json"));
}
