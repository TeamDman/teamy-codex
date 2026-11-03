//! Headless tray owner: the OTLP receiver and retained history outlive the UI child.
use super::terminal_capture::TerminalCapture;
use super::terminal_view::{TelemetryCursor, write_event};
use super::tray_app::{EventSource, TrayControl, TrayOptions};
use super::tray_visibility::{SHOW_PROBE_KIND, Visibility};
use super::ui_process::{ChildConsole, ChildEvent, History, Message, Supervisor};
use super::windows_foundation::{DesktopBackend, TrayAction};
use eyre::{Context, Result};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teamy_tracing::telemetry::otlp_http::{Receiver, ReceiverLimits};
use teamy_tracing::telemetry::{RecordStore, TelemetryRecord};

/// Retain a typed event for a later child; oversized records are counted by History.
/// # Errors
/// Returns an error if event serialization fails.
pub fn retain_event(history: &mut History, record: &TelemetryRecord) -> Result<bool> {
    let json = owned_facet_json::to_string(record)
        .map_err(|error| eyre::eyre!("encode UI event: {error}"))?;
    let mut payload = Vec::with_capacity(json.len() + 1);
    payload.push(b'E');
    payload.extend_from_slice(json.as_bytes());
    Ok(history.push(payload))
}

fn retain_text(history: &mut History, kind: u8, text: &str) {
    let mut payload = Vec::with_capacity(text.len() + 1);
    payload.push(kind);
    payload.extend_from_slice(text.as_bytes());
    let _ = history.push(payload);
}

fn close_child(child: &mut Supervisor, visibility: &mut Visibility, history: &mut History) {
    let generation = child.generation();
    if let Err(error) = child.shutdown(Duration::from_millis(250)) {
        retain_text(history, b'L', &format!("UI cleanup: {error}"));
    }
    if !child.is_running() {
        visibility.disconnected(generation);
    }
}

fn flush_remaining(
    store: &Mutex<RecordStore>,
    cursor: &mut TelemetryCursor,
    file: &mut Option<BufWriter<std::fs::File>>,
) -> Result<()> {
    let remaining = {
        let store = store
            .lock()
            .map_err(|_error| eyre::eyre!("telemetry store poisoned"))?;
        cursor.take(&store)
    };
    for record in &remaining {
        if let Some(file) = file.as_mut() {
            write_event(&mut *file, record)?;
        }
    }
    if let Some(file) = file.as_mut() {
        file.flush()?;
    }
    Ok(())
}

/// Queue retained records without blocking; the cursor advances only after enqueue.
/// # Errors
/// Returns errors other than bounded-queue backpressure.
pub fn replay_available(
    history: &History,
    child: &mut Supervisor,
    cursor: &mut u64,
    gap: &mut Option<(u64, u64)>,
) -> io::Result<()> {
    let stats = history.stats();
    let current = (stats.evicted, stats.rejected);
    if *gap != Some(current) {
        match child.queue(Message::Gap {
            generation: child.generation(),
            evicted: current.0,
            rejected: current.1,
        }) {
            Ok(()) => *gap = Some(current),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    let after = *cursor;
    for entry in history.iter().filter(|entry| entry.sequence > after) {
        match child.queue(Message::Record {
            generation: child.generation(),
            sequence: entry.sequence,
            payload: entry.payload.clone(),
        }) {
            Ok(()) => *cursor = entry.sequence,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Run the owner until tray Exit; child exit or UI failure does not stop the receiver.
/// # Errors
/// Returns bind, file, tray or receiver shutdown failures.
pub async fn run<B: DesktopBackend, E: EventSource>(
    options: &TrayOptions,
    capture: &TerminalCapture,
    control: &mut TrayControl<B, E>,
) -> Result<()> {
    let mut file = options
        .events_file
        .as_deref()
        .map(|path| OpenOptions::new().write(true).create_new(true).open(path))
        .transpose()?
        .map(BufWriter::new);
    let executable = std::env::current_exe()?.with_file_name("teamy-codex-ui.exe");
    let mut history = History::new(1000, 8 * 1024 * 1024)?;
    let store = Arc::new(Mutex::new(RecordStore::default()));
    let receiver = Receiver::bind_loopback(
        options.port.unwrap_or(4318),
        ReceiverLimits::default(),
        Arc::clone(&store),
    )
    .await?;
    let address = format!("http://{}/v1/logs", receiver.address());
    let mut cursor = TelemetryCursor::default();
    let mut child = Supervisor::default();
    let mut sent = 0u64;
    let mut gap = None;
    let mut visibility = Visibility::default();
    visibility.show(Instant::now());
    let mut closing = false;
    let mut source_status = String::new();
    let mut last_dropped = 0u64;
    let run = async {
        loop {
            for _ in 0..16 {
                match child.poll() {
                    Ok(Some(ChildEvent::Message(Message::Close { .. }))) => { closing = true; },
                    Ok(Some(ChildEvent::Message(Message::Intent { generation, kind: SHOW_PROBE_KIND, payload }))) if payload.len() == 8 => { let mut bytes = [0; 8]; bytes.copy_from_slice(&payload); visibility.confirmed(generation, u64::from_be_bytes(bytes)); },
                    Ok(Some(ChildEvent::Message(Message::Intent { .. }))) => { retain_text(&mut history, b'L', "Unsupported UI intent"); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); },
                    Ok(Some(ChildEvent::Message(_))) => {},
                    Ok(Some(ChildEvent::Exited(status))) => { if !status.success() { retain_text(&mut history, b'L', &format!("UI child exited: {status}")); } visibility.disconnected(child.generation()); sent = 0; gap = None; closing = false; },
                    Ok(Some(ChildEvent::Disconnected)) => { close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); },
                    Ok(Some(ChildEvent::Failed(error))) => { retain_text(&mut history, b'L', &error); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); },
                    Ok(None) => break,
                    Err(error) => { retain_text(&mut history, b'L', &format!("UI protocol: {error}")); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); break; },
                }
            }
            for _ in 0..16 {
                match control.next_action()? {
                    Some(TrayAction::Exit) => { visibility.stop(); return Ok::<(), eyre::Report>(()); },
                    Some(TrayAction::Hide) => { visibility.hide(); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); },
                    Some(TrayAction::Show) => visibility.show(Instant::now()),
                    None => break,
                }
            }
            if visibility.wants_child() && !child.is_running() {
                if visibility.claim_spawn() {
                    match child.spawn(&executable, &[OsString::from(&address)], ChildConsole::NewConsole) { Ok(_) => visibility.started(Instant::now()), Err(error) => { retain_text(&mut history, b'L', &format!("UI start: {error}")); visibility.hide(); } }
                } else {
                    retain_text(&mut history, b'L', "UI could not be shown; select Show to retry");
                    visibility.hide();
                }
            }
            if child.is_ready() && !closing && let Some(request) = visibility.probe_needed() {
                let generation = child.generation();
                match child.queue(Message::AppEvent { generation, kind: SHOW_PROBE_KIND, payload: request.to_be_bytes().to_vec() }) {
                    Ok(()) => visibility.probe_sent(generation, request),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {},
                    Err(error) => { retain_text(&mut history, b'L', &format!("UI show probe: {error}")); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running(); },
                }
            }
            if visibility.timed_out(Instant::now()) {
                retain_text(&mut history, b'L', "UI show probe timed out");
                close_child(&mut child, &mut visibility, &mut history);
                sent = 0; gap = None; closing = child.is_running();
            }
            let (records, stats) = {
                let store = store.lock().map_err(|_error| eyre::eyre!("telemetry store poisoned"))?;
                (cursor.take(&store), store.stats())
            };
            for record in &records {
                if let Some(file) = file.as_mut() { write_event(&mut *file, record)?; }
                let _ = retain_event(&mut history, record)?;
            }
            if !records.is_empty() && let Some(file) = file.as_mut() { file.flush()?; }
            let summary = format!("Accepted: {} · source evicted: {} · rejected: {} · missed: {} · upstream truncated: {}", stats.accepted, stats.evicted, stats.rejected_oversize, cursor.missed, stats.upstream_truncated);
            if summary != source_status { retain_text(&mut history, b'S', &summary); source_status = summary; }
            let (pending, dropped) = capture.snapshot_limit(16)?;
            for (id, line) in pending { retain_text(&mut history, b'L', &line); capture.acknowledge(id)?; }
            if dropped != last_dropped { retain_text(&mut history, b'S', &format!("Process logs evicted: {dropped}")); last_dropped = dropped; }
            if child.is_ready() && let Err(error) = replay_available(&history, &mut child, &mut sent, &mut gap) {
                retain_text(&mut history, b'L', &format!("UI delivery: {error}")); close_child(&mut child, &mut visibility, &mut history); sent = 0; gap = None; closing = child.is_running();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }.await;
    let child_shutdown = child
        .shutdown(Duration::from_secs(1))
        .map_err(eyre::Report::from);
    let receiver_shutdown = tokio::time::timeout(Duration::from_secs(15), receiver.shutdown())
        .await
        .wrap_err("receiver shutdown timed out")
        .and_then(|result| result.map_err(eyre::Report::from));
    flush_remaining(&store, &mut cursor, &mut file)?;
    run.and(child_shutdown).and(receiver_shutdown)
}
