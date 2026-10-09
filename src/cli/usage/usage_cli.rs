use super::{records::UsageRecordsArgs, snapshots::UsageSnapshotsArgs};
use crate::cli::{
    global_args::GlobalArgs,
    output::{CliOutput, OutputFormat},
};
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct UsageArgs {
    #[facet(args::subcommand)]
    pub command: UsageCommand,
}

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[repr(u8)]
pub enum UsageCommand {
    History(super::history::UsageHistoryArgs),
    Snapshots(UsageSnapshotsArgs),
    Records(UsageRecordsArgs),
}

impl UsageArgs {
    /// # Errors
    /// Returns source, validation, serialization, or cancellation failures.
    #[expect(clippy::unused_async, reason = "shared async CLI dispatch")]
    pub async fn invoke(
        self,
        global: &GlobalArgs,
        cancellation: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation.bail_if_cancelled()?;
        if global.output_format == Some(OutputFormat::Csv) {
            eyre::bail!(
                "usage reports include scope/coverage and nested buckets; use --output-format json or text"
            );
        }
        let output = match self.command {
            UsageCommand::History(args) => args.invoke(global, &cancellation)?,
            UsageCommand::Snapshots(args) => args.invoke(global)?,
            UsageCommand::Records(args) => args.invoke()?,
        };
        cancellation.bail_if_cancelled()?;
        Ok(output)
    }
}
