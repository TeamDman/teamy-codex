use arbitrary::Arbitrary;
use eyre::Result;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

use crate::cli::output::CliOutput;

/// Receive OTLP protobuf logs on loopback and display an inline overview.
#[derive(Arbitrary, Debug, Facet, PartialEq)]
pub struct TelemetryArgs {
    /// Loopback listener port; 0 chooses an available port.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub port: Option<u16>,
    /// Create a new NDJSON file of admitted telemetry; it may contain sensitive data.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub events_file: Option<String>,
}

impl TelemetryArgs {
    /// # Errors
    /// Returns an error for bind, terminal, file or shutdown failures.
    pub async fn invoke(self, cancellation: CancellationToken) -> Result<CliOutput> {
        crate::telemetry::terminal_view::run(
            self.port.unwrap_or(4318),
            self.events_file.as_deref(),
            cancellation,
        )
        .await?;
        Ok(CliOutput::none(None))
    }
}
