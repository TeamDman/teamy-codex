#![cfg(all(windows, feature = "ui-process-fixtures"))]
use std::{
    ffi::OsString,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use teamy_codex::telemetry::tray_owner::{replay_available, retain_event};
use teamy_codex::telemetry::ui_process::{ChildConsole, ChildEvent, History, Message, Supervisor};
use teamy_tracing::telemetry::{Attribute, TelemetryRecord, TelemetryValue};

fn spawn(supervisor: &mut Supervisor, mode: &str) -> u64 {
    supervisor
        .spawn(
            Path::new(env!("CARGO_BIN_EXE_codex-ui-fixture")),
            &[OsString::from(mode)],
            ChildConsole::Hidden,
        )
        .unwrap()
}
fn wait_for(supervisor: &mut Supervisor, predicate: impl Fn(&ChildEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(event) = supervisor.poll().unwrap() {
            if predicate(&event) {
                return;
            }
            assert!(
                !matches!(event, ChildEvent::Failed(_) | ChildEvent::Exited(_)),
                "unexpected child event: {event:?}"
            );
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
}
fn event(command: &str) -> TelemetryRecord {
    TelemetryRecord {
        event_name: "codex.tool_result".into(),
        attributes: vec![Attribute {
            key: "arguments".into(),
            value: TelemetryValue::String(command.into()),
        }],
        ..TelemetryRecord::default()
    }
}

#[test]
fn owner_retains_events_while_child_is_closed_and_replays_after_reopen() {
    let mut history = History::new(3, 16_384).unwrap();
    assert!(retain_event(&mut history, &event("before close")).unwrap());
    let mut child = Supervisor::default();
    assert_eq!(spawn(&mut child, "close-ui"), 1);
    wait_for(&mut child, |event| {
        matches!(event, ChildEvent::Message(Message::Ready { .. }))
    });
    wait_for(&mut child, |event| {
        matches!(event, ChildEvent::Message(Message::Close { .. }))
    });
    wait_for(
        &mut child,
        |event| matches!(event, ChildEvent::Exited(status) if status.success()),
    );
    assert!(retain_event(&mut history, &event("while closed")).unwrap());
    assert!(history.push(b"Lsynthetic process log".to_vec()));
    assert_eq!(spawn(&mut child, "normal"), 2);
    wait_for(&mut child, |event| {
        matches!(event, ChildEvent::Message(Message::Ready { .. }))
    });
    let (mut cursor, mut gap) = (0, None);
    replay_available(&history, &mut child, &mut cursor, &mut gap).unwrap();
    assert_eq!(cursor, 3);
    wait_for(&mut child, |event| {
        matches!(event, ChildEvent::Message(Message::Ack { sequence: 3, .. }))
    });
    assert_eq!(history.stats().records, 3);
    let payload = &history.iter().next().unwrap().payload;
    assert_eq!(payload[0], b'E');
    let decoded: TelemetryRecord =
        owned_facet_json::from_str(std::str::from_utf8(&payload[1..]).unwrap()).unwrap();
    assert_eq!(decoded, event("before close"));
    child.shutdown(Duration::from_secs(2)).unwrap();
}

#[test]
fn oversized_serialized_event_reports_history_rejection() {
    let mut history = History::new(2, 1024).unwrap();
    assert!(!retain_event(&mut history, &event(&"x".repeat(2048))).unwrap());
    assert_eq!(history.stats().rejected, 1);
}
