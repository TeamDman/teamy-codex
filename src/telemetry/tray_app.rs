//! Explicit Windows tray entry point. Native baseline copied from teamy-rust-cli
//! commit 454ab86; this module contains Codex-specific routing and lifecycle.
use super::terminal_capture::TerminalCapture;
use super::terminal_view::{self, TerminalControl, TerminalState};
use super::windows_foundation::native::{
    NativeDesktopBackend, OwnedConsole, TrayEvent, TrayEvents,
};
use super::windows_foundation::{ConsoleState, DesktopBackend, DesktopSession, TrayAction};
use crate::cli::global_args::GlobalArgs;
use eyre::{Context, Result, bail};
use std::ffi::OsString;
use std::io;
use teamy_cancellation::CancellationToken;

#[derive(Debug, Default, PartialEq)]
pub struct TrayOptions {
    pub port: Option<u16>,
    pub events_file: Option<String>,
    pub log_file: Option<String>,
}

impl TrayOptions {
    /// Parse the explicit tray executable's small argument surface.
    /// # Errors
    /// Returns an error for unknown, repeated, missing or non-Unicode arguments.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let arg = arg
                .into_string()
                .map_err(|_argument| eyre::eyre!("tray argument is not Unicode"))?;
            let value = args
                .next()
                .ok_or_else(|| eyre::eyre!("missing value for {arg}"))?
                .into_string()
                .map_err(|_value| eyre::eyre!("tray argument value is not Unicode"))?;
            match arg.as_str() {
                "--port" if result.port.is_none() => {
                    result.port = Some(value.parse().wrap_err("invalid port")?);
                }
                "--events-file" if result.events_file.is_none() => result.events_file = Some(value),
                "--log-file" if result.log_file.is_none() => result.log_file = Some(value),
                _ => bail!("unknown or repeated tray option: {arg}"),
            }
        }
        Ok(result)
    }
}

pub trait EventSource {
    /// # Errors
    /// Returns an error when the event channel disconnects.
    fn next_event(&mut self) -> io::Result<Option<TrayEvent>>;
    fn dropped(&self) -> u64;
}

impl EventSource for TrayEvents {
    fn next_event(&mut self) -> io::Result<Option<TrayEvent>> {
        match self.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(io::Error::other("tray event channel disconnected"))
            }
        }
    }
    fn dropped(&self) -> u64 {
        Self::dropped(self)
    }
}

/// Routes typed tray actions into the same owned-console state used for cleanup.
#[derive(Debug)]
pub struct TrayControl<B, E> {
    session: DesktopSession<B>,
    events: E,
}

impl<B: DesktopBackend, E: EventSource> TrayControl<B, E> {
    #[must_use]
    pub fn new(backend: B, events: E, state: ConsoleState) -> Self {
        Self {
            session: DesktopSession::new(backend, state),
            events,
        }
    }
    /// # Errors
    /// Returns the backend error if the explicit tray startup fails.
    pub fn activate(&mut self) -> io::Result<()> {
        self.session.activate()
    }
    /// # Errors
    /// Returns any console restoration or tray removal failure.
    pub fn shutdown(&mut self) -> io::Result<()> {
        self.session.shutdown()
    }
    #[must_use]
    pub fn into_backend(self) -> B {
        self.session.into_backend()
    }
}

impl<B: DesktopBackend, E: EventSource> TerminalControl for TrayControl<B, E> {
    fn poll(&mut self) -> Result<TerminalState> {
        if self.events.dropped() != 0 {
            bail!("tray event queue overflow");
        }
        for _ in 0..16 {
            match self.events.next_event()? {
                Some(TrayEvent::Action(TrayAction::Exit)) => return Ok(TerminalState::Exit),
                Some(TrayEvent::Action(action)) => {
                    self.session.handle(action)?;
                }
                Some(TrayEvent::Error(error)) => bail!("tray backend: {error}"),
                None => break,
            }
        }
        match self.session.console() {
            ConsoleState::OwnedVisible => Ok(TerminalState::Visible),
            ConsoleState::OwnedHidden => Ok(TerminalState::Hidden),
            ConsoleState::SharedOrConPty | ConsoleState::Unavailable => {
                bail!("tray telemetry requires an app-owned console")
            }
        }
    }
}

/// Run the dedicated, explicitly launched tray executable.
/// # Errors
/// Returns startup, receiver, logging or cleanup failures.
pub fn main() -> Result<()> {
    let options = TrayOptions::parse(std::env::args_os().skip(1))?;
    let console = OwnedConsole::allocate().wrap_err("allocate tray console")?;
    let (backend, events) = NativeDesktopBackend::new(Some(console), 64)?;
    let mut control = TrayControl::new(backend, events, ConsoleState::OwnedVisible);
    let capture = TerminalCapture::install();
    let result = (|| {
        let cancellation = CancellationToken::new();
        let global_args = GlobalArgs {
            log_file: options.log_file,
            ..GlobalArgs::default()
        };
        crate::logging_init::init_logging_with_capture(
            &global_args,
            cancellation.clone(),
            Some(capture.clone()),
        )?;
        control.activate().wrap_err("start tray")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        runtime.block_on(terminal_view::run_controlled(
            options.port.unwrap_or(4318),
            options.events_file.as_deref(),
            cancellation,
            &mut control,
        ))
    })();
    let shutdown = control.shutdown().wrap_err("restore console and stop tray");
    if let Err(error) = &result {
        tracing::error!(error = %error, "tray application failed");
    }
    if let Err(error) = &shutdown {
        tracing::error!(error = %error, "tray shutdown failed");
    }
    let replay = capture
        .finish(std::io::stderr())
        .wrap_err("replay pending process logs");
    let mut backend = control.into_backend();
    let release = if shutdown.is_ok() {
        backend
            .release_console()
            .wrap_err("release app-owned console")
    } else {
        Ok(())
    };
    let errors = [
        ("application", result),
        ("shutdown", shutdown),
        ("replay", replay),
        ("console", release),
    ]
    .into_iter()
    .filter_map(|(stage, result)| result.err().map(|error| format!("{stage}: {error:#}")))
    .collect::<Vec<_>>();
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}
