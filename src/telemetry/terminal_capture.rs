//! Bounded process-local human logs while an inline terminal owns stderr.
use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};
use tracing_subscriber::fmt::MakeWriter;

const CAPACITY: usize = 512;
const MAX_RECORD_BYTES: usize = 8192;
static CAPTURE: OnceLock<TerminalCapture> = OnceLock::new();

#[derive(Clone, Debug, Default)]
pub struct TerminalCapture(Arc<Mutex<State>>);

#[derive(Debug)]
struct State {
    active: bool,
    next: u64,
    dropped: u64,
    records: VecDeque<(u64, String)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            active: true,
            next: 0,
            dropped: 0,
            records: VecDeque::new(),
        }
    }
}

impl TerminalCapture {
    pub fn install() -> Self {
        CAPTURE.get_or_init(Self::default).clone()
    }
    pub fn current() -> Option<Self> {
        CAPTURE.get().cloned()
    }

    /// Read pending records without acknowledging them.
    ///
    /// # Errors
    /// Returns an error when the buffer lock is poisoned.
    pub fn snapshot(&self) -> io::Result<(Vec<(u64, String)>, u64)> {
        self.snapshot_limit(usize::MAX)
    }

    /// Read at most `limit` pending records without acknowledging them.
    ///
    /// # Errors
    /// Returns an error when the buffer lock is poisoned.
    pub fn snapshot_limit(&self, limit: usize) -> io::Result<(Vec<(u64, String)>, u64)> {
        let state = self
            .0
            .lock()
            .map_err(|_poisoned| io::Error::other("terminal log buffer poisoned"))?;
        Ok((
            state.records.iter().take(limit).cloned().collect(),
            state.dropped,
        ))
    }
    /// Acknowledge only after the caller has successfully printed these records.
    ///
    /// # Errors
    /// Returns an error when the buffer lock is poisoned.
    pub fn acknowledge(&self, through: u64) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_poisoned| io::Error::other("terminal log buffer poisoned"))?;
        while state.records.front().is_some_and(|(id, _)| *id <= through) {
            state.records.pop_front();
        }
        Ok(())
    }
    /// Restore ordinary stderr logging and replay records not yet displayed.
    ///
    /// # Errors
    /// Returns an error when the buffer is poisoned or the writer fails.
    pub fn finish(&self, mut writer: impl Write) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_poisoned| io::Error::other("terminal log buffer poisoned"))?;
        state.active = false;
        while let Some((_, record)) = state.records.front() {
            writer.write_all(record.as_bytes())?;
            state.records.pop_front();
        }
        writer.flush()
    }
}

#[derive(Debug)]
pub struct CaptureWriter {
    capture: TerminalCapture,
    bytes: Vec<u8>,
    truncated: bool,
}

impl<'a> MakeWriter<'a> for TerminalCapture {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter {
            capture: self.clone(),
            bytes: Vec::new(),
            truncated: false,
        }
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let available = MAX_RECORD_BYTES.saturating_sub(self.bytes.len());
        let count = buf.len().min(available);
        self.bytes.extend_from_slice(&buf[..count]);
        self.truncated |= count < buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let mut message = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            message.push_str(" [log truncated]");
        }
        if let Ok(mut state) = self.capture.0.lock() {
            if !state.active {
                drop(state);
                let _ = io::stderr().write_all(message.as_bytes());
                return;
            }
            if state.records.len() == CAPACITY {
                state.records.pop_front();
                state.dropped = state.dropped.saturating_add(1);
            }
            state.next = state.next.saturating_add(1);
            let id = state.next;
            state.records.push_back((id, message));
        }
    }
}
