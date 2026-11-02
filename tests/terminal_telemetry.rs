#![cfg(feature = "telemetry")]

use std::io::Write;
use teamy_codex::telemetry::terminal_capture::TerminalCapture;
use teamy_codex::telemetry::terminal_view::{
    TelemetryCursor, detail_rows, detail_text, event_summary, print_above, write_event,
};
use teamy_tracing::telemetry::{
    Attribute, RecordStore, StoreLimits, TelemetryRecord, TelemetryValue,
};
use tracing_subscriber::fmt::MakeWriter;

struct Broken;
impl Write for Broken {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("synthetic sink failure"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn result_record() -> TelemetryRecord {
    TelemetryRecord {
        event_name: "codex.tool_result".into(),
        attributes: vec![
            Attribute {
                key: "tool_name".into(),
                value: TelemetryValue::String("exec_command".into()),
            },
            Attribute {
                key: "output_truncated".into(),
                value: TelemetryValue::Bool(true),
            },
            Attribute {
                key: "arguments".into(),
                value: TelemetryValue::String(r#"{"cmd":"Get-Date","shell":"powershell"}"#.into()),
            },
        ],
        ..TelemetryRecord::default()
    }
}

#[test]
fn cursor_counts_loss_and_does_not_repeat_records() {
    let mut store = RecordStore::new(StoreLimits {
        max_records: 1,
        max_logical_bytes: 4096,
    });
    assert!(store.push(result_record()));
    assert!(store.push(result_record()));
    let mut cursor = TelemetryCursor::default();
    assert_eq!(cursor.take(&store).len(), 1);
    assert_eq!(
        (
            cursor.missed,
            cursor.results,
            store.stats().upstream_truncated
        ),
        (1, 1, 2)
    );
    assert!(cursor.take(&store).is_empty());
    assert!(event_summary(&result_record()).contains("output truncated"));
}

#[test]
fn selection_uses_stable_sequence_and_marks_evicted_records() {
    use teamy_codex::telemetry::terminal_navigation::{Navigation, Screen};
    let mut store = RecordStore::new(StoreLimits {
        max_records: 2,
        max_logical_bytes: 8192,
    });
    assert!(store.push(result_record()));
    assert!(store.push(result_record()));
    let mut navigation = Navigation::default();
    navigation.initialize_selection(store.stats());
    assert_eq!(navigation.selected, Some(2));
    navigation.previous(store.stats());
    assert_eq!(navigation.selected_index(store.stats()), Some(0));
    navigation.enter_detail(store.stats());
    navigation.next(store.stats());
    assert_eq!(navigation.screen, Screen::Detail { scroll: 1 });
    assert!(store.push(result_record()));
    assert_eq!(navigation.selected_index(store.stats()), None);
    assert!(!navigation.back());
    navigation.next(store.stats());
    assert!(navigation.selected_index(store.stats()).is_some());
    assert!(navigation.back());
}

#[test]
fn details_preserve_long_arguments_and_escape_controls() {
    let mut record = result_record();
    record.attributes.push(Attribute {
        key: "output".into(),
        value: TelemetryValue::String(format!("{}\u{1b}\u{202e}\tend", "x".repeat(10_000))),
    });
    let text = detail_text(&record);
    assert!(text.contains("Get-Date"));
    assert!(text.contains("output_truncated"));
    assert!(text.contains("\\u{1b}\\u{202e}"));
    assert!(!text.contains('\u{1b}'));
    let rows = detail_rows(&text, 30);
    assert!(rows.len() > 300);
    assert!(rows.iter().any(|line| line.contains("\\tend")));
}

#[test]
fn process_logs_are_acknowledged_only_after_delivery() {
    let capture = TerminalCapture::default();
    let mut writer = capture.make_writer();
    write!(writer, "first").unwrap();
    drop(writer);
    let mut writer = capture.make_writer();
    write!(writer, "second").unwrap();
    drop(writer);
    let (records, _) = capture.snapshot().unwrap();
    assert_eq!(records.len(), 2);
    capture.acknowledge(records[0].0).unwrap();
    let mut tail = Vec::new();
    capture.finish(&mut tail).unwrap();
    assert_eq!(tail, b"second");
}

#[test]
fn process_log_buffer_reports_eviction_and_truncation() {
    let capture = TerminalCapture::default();
    for _ in 0..513 {
        let mut writer = capture.make_writer();
        writer.write_all(b"entry").unwrap();
    }
    let (records, dropped) = capture.snapshot().unwrap();
    assert_eq!((records.len(), dropped), (512, 1));
    let mut writer = capture.make_writer();
    writer.write_all(&vec![b'x'; 9000]).unwrap();
    drop(writer);
    let (records, dropped) = capture.snapshot().unwrap();
    assert_eq!(dropped, 2);
    assert!(records.last().unwrap().1.ends_with(" [log truncated]"));
}

#[test]
fn event_file_is_facet_ndjson_and_returns_write_failures() {
    let record = result_record();
    let mut bytes = Vec::new();
    write_event(&mut bytes, &record).unwrap();
    assert_eq!(bytes.last(), Some(&b'\n'));
    let decoded: TelemetryRecord =
        owned_facet_json::from_str(std::str::from_utf8(&bytes).unwrap()).unwrap();
    assert_eq!(decoded, record);
    assert!(write_event(Broken, &record).is_err());
}

#[test]
fn inline_backend_inserts_safe_log_above_view() {
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::{Terminal, TerminalOptions, Viewport};
    let mut terminal = Terminal::with_options(
        TestBackend::new(48, 8),
        TerminalOptions {
            viewport: Viewport::Inline(3),
        },
    )
    .unwrap();
    terminal
        .draw(|frame| frame.render_widget(Paragraph::new("overview"), frame.area()))
        .unwrap();
    print_above(&mut terminal, "log\u{1b}[31m line").unwrap();
    terminal
        .draw(|frame| frame.render_widget(Paragraph::new("overview"), frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let visible = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("log\\u{1b}[31m line"));
    assert!(visible.contains("overview"));
}

#[test]
fn loopback_receiver_accepts_synthetic_protobuf_and_shuts_down() {
    use std::sync::{Arc, Mutex};
    use teamy_tracing::telemetry::otlp_http::{Receiver, ReceiverLimits};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Arc::new(Mutex::new(RecordStore::default()));
        let receiver = Receiver::bind_loopback(0, ReceiverLimits::default(), Arc::clone(&store)).await.unwrap();
        let address = receiver.address();
        assert!(address.ip().is_loopback());
        let name = b"codex.tool_result";
        let mut record = vec![0x62, u8::try_from(name.len()).unwrap()];
        record.extend_from_slice(name);
        let mut scope = vec![0x12, u8::try_from(record.len()).unwrap()]; scope.extend(record);
        let mut resource = vec![0x12, u8::try_from(scope.len()).unwrap()]; resource.extend(scope);
        let mut request = vec![0x0a, u8::try_from(resource.len()).unwrap()]; request.extend(resource);
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let headers = format!("POST /v1/logs HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", request.len());
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(&request).await.unwrap();
        let mut response = Vec::new(); socket.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        let mut cursor = TelemetryCursor::default();
        assert_eq!(cursor.take(&store.lock().unwrap()).len(), 1);
        assert_eq!(cursor.results, 1);
        receiver.shutdown().await.unwrap();
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        drop(listener);
    });
}
