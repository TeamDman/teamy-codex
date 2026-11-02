//! Codex-specific presentation data. This module does not receive telemetry.
mod correlate;
mod detail;
mod event;
#[cfg(feature = "telemetry")]
pub mod terminal_capture;
#[cfg(feature = "telemetry")]
mod terminal_foundation;
#[cfg(feature = "telemetry")]
pub mod terminal_navigation;
#[cfg(feature = "telemetry")]
pub mod terminal_view;
mod tool_details;
#[cfg(all(windows, feature = "windows-tray"))]
pub mod tray_app;
#[cfg(all(windows, feature = "windows-tray"))]
pub mod windows_foundation;

pub use correlate::{Correlation, ToolCallRow, correlate};
pub use detail::{CommandDetail, ShellKind, command_detail, escape_display};
pub use event::{CodexEvent, Field, FieldValue};
pub use tool_details::{ToolDecision, ToolResult, tool_decision, tool_result};
