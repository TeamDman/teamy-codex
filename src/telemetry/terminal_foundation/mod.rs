//! Copied from teamy-rust-cli commit 9cbff90; only inline ownership and safe
//! scrollback are adopted here. Process-log capture remains application-specific.
mod inline_terminal;
mod text;
pub use inline_terminal::{InlineTerminal, print_above};
use text::escape_terminal_text;
