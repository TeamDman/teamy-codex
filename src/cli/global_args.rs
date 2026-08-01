use crate::cli::output::OutputFormat;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::StopAfterArgs;

#[derive(Facet, Arbitrary, Debug, Default, PartialEq)]
pub struct GlobalArgs {
    /// Override the Codex home directory. Defaults to `%USERPROFILE%\\.codex`.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_home: Option<String>,

    /// Enable debug logging.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub debug: bool,

    /// Log level filter directive.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub log_filter: Option<String>,

    /// Write structured NDJSON logs to a file or directory.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub log_file: Option<String>,

    /// Stop after a duration, span, or Tracy frame condition.
    #[facet(flatten, default)]
    #[arbitrary(default)]
    pub stop_after: StopAfterArgs,

    /// Render results as text, JSON, or CSV.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub output_format: Option<OutputFormat>,
}
