#![cfg(all(windows, feature = "windows-tray"))]

use std::{cell::RefCell, collections::VecDeque, ffi::OsString, io, rc::Rc};
use teamy_codex::telemetry::terminal_view::{TerminalControl, TerminalState};
use teamy_codex::telemetry::tray_app::{EventSource, TrayControl, TrayOptions};
use teamy_codex::telemetry::windows_foundation::native::TrayEvent;
use teamy_codex::telemetry::windows_foundation::{ConsoleState, DesktopBackend, TrayAction};

#[derive(Clone, Debug, Default)]
struct Backend(Rc<RefCell<Vec<&'static str>>>);
impl DesktopBackend for Backend {
    fn create_tray(&mut self) -> io::Result<()> {
        self.0.borrow_mut().push("create");
        Ok(())
    }
    fn remove_tray(&mut self) -> io::Result<()> {
        self.0.borrow_mut().push("remove");
        Ok(())
    }
    fn set_owned_console_visible(&mut self, visible: bool) -> io::Result<()> {
        self.0
            .borrow_mut()
            .push(if visible { "show" } else { "hide" });
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Events {
    queue: Rc<RefCell<VecDeque<TrayEvent>>>,
    dropped: u64,
}
impl EventSource for Events {
    fn next_event(&mut self) -> io::Result<Option<TrayEvent>> {
        Ok(self.queue.borrow_mut().pop_front())
    }
    fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[test]
fn routes_visibility_exit_and_restores_on_shutdown() {
    let backend = Backend::default();
    let events = Events::default();
    let queue = Rc::clone(&events.queue);
    let mut control = TrayControl::new(backend.clone(), events, ConsoleState::OwnedVisible);
    control.activate().unwrap();
    queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Hide));
    assert_eq!(control.poll().unwrap(), TerminalState::Hidden);
    queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Show));
    assert_eq!(control.poll().unwrap(), TerminalState::Visible);
    queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Hide));
    queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Exit));
    queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Hide));
    assert_eq!(control.poll().unwrap(), TerminalState::Exit);
    control.shutdown().unwrap();
    assert_eq!(
        *backend.0.borrow(),
        ["create", "hide", "show", "hide", "show", "remove"]
    );
    assert_eq!(queue.borrow().len(), 1);
}

#[test]
fn refuses_shared_console_and_reports_lost_events() {
    let backend = Backend::default();
    let events = Events::default();
    events
        .queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Hide));
    let mut control = TrayControl::new(backend.clone(), events, ConsoleState::SharedOrConPty);
    let _ = control.poll().unwrap_err();
    assert!(backend.0.borrow().is_empty());
    let events = Events {
        dropped: 1,
        ..Events::default()
    };
    let mut control = TrayControl::new(backend, events, ConsoleState::OwnedVisible);
    assert!(control.poll().unwrap_err().to_string().contains("overflow"));
}

#[test]
fn tray_failure_after_hide_still_restores_on_shutdown() {
    let backend = Backend::default();
    let events = Events::default();
    events
        .queue
        .borrow_mut()
        .push_back(TrayEvent::Action(TrayAction::Hide));
    events
        .queue
        .borrow_mut()
        .push_back(TrayEvent::Error("synthetic restart failure".into()));
    let mut control = TrayControl::new(backend.clone(), events, ConsoleState::OwnedVisible);
    control.activate().unwrap();
    assert!(
        control
            .poll()
            .unwrap_err()
            .to_string()
            .contains("synthetic restart failure")
    );
    control.shutdown().unwrap();
    assert_eq!(*backend.0.borrow(), ["create", "hide", "show", "remove"]);
}

#[test]
fn tray_arguments_require_explicit_valid_values() {
    let args = [
        "--port",
        "0",
        "--events-file",
        "events.ndjson",
        "--log-file",
        "logs.ndjson",
    ]
    .into_iter()
    .map(OsString::from);
    let options = TrayOptions::parse(args).unwrap();
    assert_eq!(options.port, Some(0));
    assert_eq!(options.events_file.as_deref(), Some("events.ndjson"));
    assert_eq!(options.log_file.as_deref(), Some("logs.ndjson"));
    let _ = TrayOptions::parse([OsString::from("--port")]).unwrap_err();
    let _ = TrayOptions::parse(
        ["--port", "1", "--port", "2"]
            .into_iter()
            .map(OsString::from),
    )
    .unwrap_err();
}
