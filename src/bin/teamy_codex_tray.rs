#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() -> eyre::Result<()> {
    teamy_codex::telemetry::tray_app::main()
}

#[cfg(not(windows))]
fn main() -> eyre::Result<()> {
    eyre::bail!("the tray application requires Windows")
}
