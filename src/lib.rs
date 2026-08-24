#![deny(clippy::disallowed_methods)]
#![deny(clippy::disallowed_macros)]
#![deny(clippy::disallowed_types)]

mod backup;
pub mod cli;
mod codex_state;
mod inventory;
pub mod logging_init;
#[cfg(windows)]
mod windows_startup;

use crate::cli::Cli;
use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use teamy_cancellation::CtrlCHandler;

fn version() -> String {
    let built_at = option_env!("BUILD_TIMESTAMP_UNIX")
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .map_or_else(
            || "unknown build time".to_owned(),
            |timestamp| {
                timestamp
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S %Z")
                    .to_string()
            },
        );

    format!(
        "{} (repo {}, branch {}, rev {}, worktree {}, built {})",
        env!("CARGO_PKG_VERSION"),
        env!("GIT_REPOSITORY_URL"),
        env!("GIT_BRANCH"),
        env!("GIT_REVISION"),
        env!("GIT_WORKTREE_STATUS"),
        built_at,
    )
}

/// Run the Teamy Codex CLI.
///
/// # Errors
///
/// Returns an error when logging, cancellation, command execution, or output
/// rendering fails.
///
/// # Panics
///
/// Panics if the CLI schema is invalid.
pub fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    let cancellation_token = CtrlCHandler::default().install()?;

    #[cfg(windows)]
    {
        let _ = windows_startup::enable_ansi_support();
        windows_startup::warn_if_utf8_not_enabled();
    };

    let version = version();
    let cli: Cli = figue::Driver::new(
        figue::builder::<Cli>()
            .expect("schema should be valid")
            .cli(move |cli| cli.args_os(std::env::args_os().skip(1)).strict())
            .help(move |help| {
                help.version(version)
                    .include_implementation_source_file(true)
            })
            .build(),
    )
    .run()
    .unwrap();

    let _stop_after_duration_thread = cli
        .global_args
        .stop_after
        .start_stop_after_duration_thread(cancellation_token.clone())?;

    logging_init::init_logging(&cli.global_args, cancellation_token.clone())?;

    let requested_output_format = cli.global_args.output_format;
    let output = cli.invoke(cancellation_token.clone())?;
    cancellation_token.bail_if_cancelled()?;
    output.emit(requested_output_format)?;
    cancellation_token.bail_if_cancelled()?;
    Ok(())
}
