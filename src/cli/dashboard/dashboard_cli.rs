use crate::active_threads::{ActiveThreadsReport, ActiveThreadsRequest, clean_text};
use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_app_server_process::observe_threads;
use arbitrary::Arbitrary;
use crossterm::{cursor, event, execute, terminal};
use eyre::bail;
use facet::Facet;
use figue as args;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct DashboardArgs {
    /// Refresh interval in seconds, at least 1. Defaults to 3.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub refresh_seconds: Option<u64>,
    /// Codex executable to use for the shared-runtime proxy.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_bin: Option<String>,
    /// Explicit shared-runtime control socket, passed to the supported proxy.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub socket: Option<String>,
    /// Use an isolated metadata server; lifecycle will remain unknown.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub isolated: bool,
    /// Include idle and unknown threads as well as active threads.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub show_all: bool,
    /// Print one frame without terminal control (useful for smoke checks).
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub once: bool,
}

struct TerminalGuard;

struct WorkerGuard(CancellationToken);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.request_cancel("dashboard observer closed");
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(
            std::io::stdout(),
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
    }
}

impl DashboardArgs {
    /// # Errors
    /// Returns an error for invalid options, terminal failures, or cancellation.
    pub async fn invoke(
        self,
        global: &GlobalArgs,
        cancellation: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        let refresh = self.refresh_seconds.unwrap_or(3);
        if !(1..=3600).contains(&refresh) {
            bail!("--refresh-seconds must be between 1 and 3600");
        }
        if self.isolated && self.socket.is_some() {
            bail!("--socket conflicts with --isolated");
        }
        if !self.once && (!std::io::stdout().is_terminal() || !std::io::stdin().is_terminal()) {
            bail!("dashboard requires a terminal; use --once for a plain frame");
        }
        let _guard = enter_terminal(self.once)?;
        let worker_cancellation = CancellationToken::new();
        let _worker_guard = WorkerGuard(worker_cancellation.clone());
        let mut task = None;
        let mut previous_frame = String::new();
        let mut report = None;
        let mut next_refresh = Instant::now();
        loop {
            if cancellation.bail_if_cancelled().is_err() {
                break;
            }
            if task.is_none() && Instant::now() >= next_refresh {
                let bin = self.codex_bin.clone();
                let home = global.codex_home.clone();
                let socket = self.socket.clone();
                let request = ActiveThreadsRequest {
                    active_only: !self.show_all,
                    limit: Some(100),
                    isolated: self.isolated,
                    archive_filter: crate::active_threads::ArchiveFilter::NonArchived,
                };
                let token = worker_cancellation.clone();
                task = Some(tokio::task::spawn_blocking(move || {
                    observe_threads(
                        bin.as_deref(),
                        home.as_deref(),
                        socket.as_deref(),
                        request,
                        &token,
                    )
                }));
            }
            if task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
                && let Some(finished) = task.take()
            {
                report = Some(finished.await??);
                next_refresh = Instant::now() + Duration::from_secs(refresh);
            }
            let (width, height) = terminal::size().unwrap_or((100, 30));
            let frame = render(
                report.as_ref(),
                usize::from(width),
                usize::from(height),
                task.is_some(),
            );
            if self.once {
                if report.is_some() {
                    std::io::stdout().write_all(frame.as_bytes())?;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            if frame != previous_frame {
                execute!(
                    std::io::stdout(),
                    cursor::MoveTo(0, 0),
                    terminal::Clear(terminal::ClearType::All)
                )?;
                std::io::stdout().write_all(frame.replace('\n', "\r\n").as_bytes())?;
                std::io::stdout().flush()?;
                previous_frame = frame;
            }
            if poll_quit()? {
                break;
            }
        }
        // Cancel only our owned proxy/child; never the observed runtime or its turns.
        worker_cancellation.request_cancel("dashboard closed");
        if let Some(task) = task {
            let _ = task.await;
        }
        Ok(CliOutput::none(None))
    }
}

fn enter_terminal(once: bool) -> eyre::Result<Option<TerminalGuard>> {
    if once {
        return Ok(None);
    }
    terminal::enable_raw_mode()?;
    let guard = TerminalGuard;
    execute!(
        std::io::stdout(),
        terminal::EnterAlternateScreen,
        cursor::Hide
    )?;
    Ok(Some(guard))
}

fn poll_quit() -> eyre::Result<bool> {
    if event::poll(Duration::from_millis(100))?
        && let event::Event::Key(key) = event::read()?
    {
        return Ok(key.kind != event::KeyEventKind::Release
            && (matches!(key.code, event::KeyCode::Char('q') | event::KeyCode::Esc)
                || (key.code == event::KeyCode::Char('c')
                    && key.modifiers.contains(event::KeyModifiers::CONTROL))));
    }
    Ok(false)
}

fn render(
    report: Option<&ActiveThreadsReport>,
    width: usize,
    height: usize,
    refreshing: bool,
) -> String {
    let width = width.saturating_sub(1);
    let mut lines = vec![format!(
        "TEAMY CODEX / DASHBOARD   q/Esc/Ctrl-C quit{}",
        if refreshing { "   refreshing..." } else { "" }
    )];
    if let Some(report) = report {
        lines.push(format!(
            "{} | {} | complete={} | unknown={} | truncated={}",
            report.coverage.source,
            report.coverage.connection,
            report.coverage.complete,
            report.coverage.unknown_threads,
            report.coverage.truncated
        ));
        lines.push(report.coverage.scope.clone());
        for issue in &report.coverage.issues {
            lines.push(format!("Coverage: {issue}"));
        }
        if report.data.is_empty() {
            lines.push(
                "No observed active rows; check coverage before concluding work is idle.".into(),
            );
        }
        for thread in &report.data {
            lines.push(format!(
                "{}  {}  {}",
                thread.status, thread.id, thread.title
            ));
            lines.push(format!(
                "  {}",
                thread.latest_command.as_ref().map_or_else(
                    || thread.latest_command_availability.clone(),
                    |command| command.display.clone()
                )
            ));
        }
    } else {
        lines.push("Connecting to selected runtime...".into());
    }
    let mut frame = lines
        .into_iter()
        .take(height.saturating_sub(1))
        .map(|line| clean_text(&line, width))
        .collect::<Vec<_>>()
        .join("\n");
    frame.push('\n');
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_and_disconnected_frames_are_bounded() {
        let report = ActiveThreadsReport::disconnected(false);
        for width in [0, 1, 20, 80] {
            let frame = render(Some(&report), width, 8, false);
            assert!(
                frame
                    .lines()
                    .all(|line| line.chars().count() < width.max(1))
            );
            assert!(frame.lines().count() <= 7);
        }
        assert!(render(Some(&report), 120, 20, false).contains("disconnected"));
    }
}
