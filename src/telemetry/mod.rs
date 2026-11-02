//! Codex-specific presentation data. This module does not receive telemetry.
mod correlate;
mod detail;
mod event;
#[cfg(feature = "telemetry")]
pub mod terminal_capture;
#[cfg(feature = "telemetry")]
pub mod terminal_view;
mod tool_details;

pub use correlate::{Correlation, ToolCallRow, correlate};
pub use detail::{CommandDetail, ShellKind, command_detail, escape_display};
pub use event::{CodexEvent, Field, FieldValue};
pub use tool_details::{ToolDecision, ToolResult, tool_decision, tool_result};
