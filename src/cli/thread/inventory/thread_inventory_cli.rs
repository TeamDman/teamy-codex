use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::resolve_codex_home;
use crate::inventory::InventoryRequest;
use crate::inventory::capture_inventory;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use std::path::PathBuf;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
pub struct ThreadInventoryArgs {
    /// Required lookback duration, such as `1week`, `48hours`, or `30min`.
    #[facet(args::named)]
    pub active_since: String,

    /// Include tasks whose working directory is under any supplied path. Repeatable.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub cwd_under: Vec<String>,

    /// Include archived tasks. Archived tasks are excluded by default.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub include_archived: bool,

    /// Explicit maximum rows to return. Omitting this option returns every match.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub limit: Option<usize>,

    /// Explicit path for the versioned audit manifest.
    #[facet(args::named)]
    pub manifest: String,

    /// Atomically replace an existing manifest at the explicit path.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub overwrite_manifest: bool,
}

impl ThreadInventoryArgs {
    /// # Errors
    ///
    /// Returns an error when the read-only inventory query, digest, provenance,
    /// or atomic manifest write fails.
    #[expect(
        clippy::unused_async,
        reason = "command invoke methods share the async CLI dispatch shape"
    )]
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        let codex_home = resolve_codex_home(global_args.codex_home.as_deref())?;
        let rows = capture_inventory(
            &codex_home,
            &InventoryRequest {
                active_since: self.active_since,
                cwd_under: self.cwd_under,
                include_archived: self.include_archived,
                limit: self.limit,
                manifest_path: PathBuf::from(self.manifest),
                overwrite_manifest: self.overwrite_manifest,
            },
        )?;
        cancellation_token.bail_if_cancelled()?;
        // Digest and manifest work: setup plus 20 ms per selected inventory row.
        let threshold = Some(
            std::time::Duration::from_secs(3).saturating_add(std::time::Duration::from_millis(
                u64::try_from(rows.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(20),
            )),
        );
        Ok(CliOutput::facet_rows(rows, threshold))
    }
}
