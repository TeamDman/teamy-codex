use crate::cli::global_args::GlobalArgs;
use crate::cli::output::CliOutput;
use crate::codex_state::list_threads;
use crate::codex_state::resolve_codex_home;
use arbitrary::Arbitrary;
use facet::Facet;
use figue as args;
use teamy_cancellation::CancellationToken;

#[derive(Facet, Arbitrary, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI options are independent switches"
)]
pub struct ThreadListArgs {
    /// Observe running/waiting threads from the shared app-server, with explicit coverage.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub active: bool,

    /// Observe all runtime metadata, including explicit idle/unknown states.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub observe: bool,

    /// Use an isolated metadata server; lifecycle remains unknown. Requires --active or --observe.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub isolated: bool,

    /// Codex executable for the observer. Requires --active or --observe.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub codex_bin: Option<String>,

    /// Explicit shared app-server control socket. Requires --active or --observe.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub socket: Option<String>,
    /// Maximum number of threads to return. Defaults to 50 for persisted listing; observer has no row limit.
    #[facet(args::named)]
    #[arbitrary(default)]
    pub limit: Option<usize>,

    /// Include archived threads.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub all: bool,

    /// Return only archived threads.
    #[facet(args::named, default)]
    #[arbitrary(default)]
    pub archived: bool,
}

impl ThreadListArgs {
    /// # Errors
    ///
    /// Returns an error when the Codex database cannot be opened or queried.
    pub async fn invoke(
        self,
        global_args: &GlobalArgs,
        cancellation_token: CancellationToken,
    ) -> eyre::Result<CliOutput> {
        cancellation_token.bail_if_cancelled()?;
        if self.limit == Some(0) {
            eyre::bail!("--limit must be greater than zero");
        }
        if self.isolated && self.socket.is_some() {
            eyre::bail!("--socket conflicts with --isolated");
        }
        if self.active || self.observe {
            let home = global_args.codex_home.clone();
            let request = crate::active_threads::ActiveThreadsRequest {
                active_only: self.active,
                limit: self.limit,
                isolated: self.isolated,
                archive_filter: if self.all {
                    crate::active_threads::ArchiveFilter::All
                } else if self.archived {
                    crate::active_threads::ArchiveFilter::Archived
                } else {
                    crate::active_threads::ArchiveFilter::NonArchived
                },
            };
            let result = tokio::task::spawn_blocking(move || {
                crate::codex_app_server_process::observe_threads(
                    self.codex_bin.as_deref(),
                    home.as_deref(),
                    self.socket.as_deref(),
                    request,
                    &cancellation_token,
                )
            })
            .await??;
            return Ok(CliOutput::facet(result, None));
        }
        if self.isolated || self.socket.is_some() || self.codex_bin.is_some() {
            eyre::bail!("observer options require --active or --observe");
        }
        // Query/rollout read and rendering: setup plus 10 ms per requested row.
        let threshold = Some(
            std::time::Duration::from_secs(2).saturating_add(std::time::Duration::from_millis(
                u64::try_from(self.limit.unwrap_or(50))
                    .unwrap_or(u64::MAX)
                    .saturating_mul(10),
            )),
        );
        let codex_home = resolve_codex_home(global_args.codex_home.as_deref())?;
        let threads = list_threads(&codex_home, &self)?;
        Ok(CliOutput::facet_rows(threads, threshold))
    }
}
