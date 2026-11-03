#[cfg(windows)]
fn main() -> eyre::Result<()> {
    teamy_codex::telemetry::tray_child::main()
}

#[cfg(not(windows))]
fn main() -> eyre::Result<()> {
    eyre::bail!("the tray UI requires Windows")
}
