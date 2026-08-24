use crate::cli::thread::list::ThreadListArgs;
use crate::codex_state::ThreadSummary;
use crate::codex_state::list_threads;
use chrono::Utc;
use eyre::Context;
use eyre::ContextCompat;
use eyre::bail;
use facet::Facet;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use teamy_cancellation::CancellationToken;
use tracing::info;

pub const DEFAULT_BACKUP_DIRECTORY: &str = r"D:\OneDrive\Documents\Backups\codex\teamy-codex";

const BACKUP_CONFIG_DIRECTORY: &str = "teamy-codex";
const BACKUP_CONFIG_FILE: &str = "backup.json";
const BACKUP_MANIFEST_FILE: &str = ".teamy-codex-backup.json";
const BACKUP_MANIFEST_VERSION: u32 = 2;
const BACKUP_THREAD_LIMIT: usize = 1_000_000;
const MAX_BACKUP_JOBS: usize = 64;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const COPY_PROGRESS_BYTES: u64 = 256 * 1024 * 1024;

const GLOBAL_FILES: [&str; 3] = ["config.toml", "history.jsonl", "session_index.jsonl"];
const GLOBAL_DIRECTORIES: [&str; 3] = ["memories", "attachments", "generated_images"];

#[derive(Facet, Debug)]
pub struct BackupDirectoryInfo {
    pub source: String,
    pub destination: String,
    pub config_path: String,
    pub configured: bool,
}

#[derive(Facet, Debug)]
pub struct BackupNowReport {
    pub source: String,
    pub destination: String,
    pub parallel_jobs: usize,
    pub sessions_scanned: usize,
    pub active_sessions: usize,
    pub archived_sessions: usize,
    pub sessions_copied: usize,
    pub sessions_skipped: usize,
    pub global_files_scanned: usize,
    pub global_files_copied: usize,
    pub global_files_skipped: usize,
    pub bytes_copied: u64,
    pub removed_sessions_retained: usize,
    pub removed_global_files_retained: usize,
    pub manifest_path: String,
    pub completed_at: String,
}

#[derive(Facet, Debug)]
pub struct BackupStatus {
    pub source: String,
    pub destination: String,
    pub config_path: String,
    pub manifest_path: String,
    pub destination_exists: bool,
    pub manifest_exists: bool,
    pub manifest_source_matches: bool,
    pub last_backup_at: Option<String>,
    pub source_sessions: usize,
    pub active_sessions: usize,
    pub archived_sessions: usize,
    pub tracked_sessions: usize,
    pub pending_sessions: usize,
    pub pending_session_bytes: u64,
    pub source_global_files: usize,
    pub tracked_global_files: usize,
    pub pending_global_files: usize,
    pub pending_global_bytes: u64,
    pub removed_sessions_retained: usize,
    pub removed_global_files_retained: usize,
}

#[derive(Facet, Debug)]
struct BackupConfig {
    destination: String,
}

#[derive(Facet, Debug)]
struct BackupManifest {
    format_version: u32,
    source: String,
    completed_at: String,
    sessions: BTreeMap<String, SessionRecord>,
    global_files: BTreeMap<String, GlobalFileRecord>,
}

#[derive(Clone, Facet, Debug)]
struct SessionRecord {
    present: bool,
    archived: bool,
    rollout: Option<FileRecord>,
}

#[derive(Clone, Facet, Debug)]
struct GlobalFileRecord {
    present: bool,
    signature: FileSignature,
}

#[derive(Clone, Facet, Debug, Eq, PartialEq)]
struct FileRecord {
    signature: FileSignature,
}

#[derive(Clone, Facet, Debug, Eq, PartialEq)]
struct FileSignature {
    size: u64,
    modified_unix_ms: Option<i64>,
}

#[derive(Debug)]
struct GlobalSourceFile {
    source: PathBuf,
    relative: String,
    signature: FileSignature,
}

#[derive(Facet, Debug)]
struct BackupSessionMetadata {
    id: String,
    title: String,
    cwd: String,
    created_at_ms: Option<i64>,
    updated_at_ms: Option<i64>,
    recency_at_ms: i64,
    archived: bool,
    tokens_used: i64,
    source: String,
    thread_source: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    rollout_file: Option<String>,
    rollout_source_signature: Option<FileSignature>,
}

#[derive(Debug)]
struct SessionBackupWork {
    session_id: String,
    archived: bool,
    metadata_path: PathBuf,
    metadata_bytes: Vec<u8>,
    metadata_changed: bool,
    rollout_source: Option<PathBuf>,
    rollout_destination: PathBuf,
    rollout_signature: Option<FileSignature>,
    rollout_needs_copy: bool,
}

#[derive(Debug)]
struct SessionBackupResult {
    work_index: usize,
    wrote_files: bool,
    bytes_copied: u64,
}

impl SessionBackupWork {
    fn needs_write(&self) -> bool {
        self.metadata_changed || self.rollout_needs_copy
    }

    fn planned_bytes(&self) -> u64 {
        let metadata_bytes = self
            .metadata_changed
            .then_some(self.metadata_bytes.len())
            .map_or(0, |bytes| u64::try_from(bytes).unwrap_or(u64::MAX));
        let rollout_bytes = if self.rollout_needs_copy {
            self.rollout_signature
                .as_ref()
                .map_or(0, |value| value.size)
        } else {
            0
        };
        metadata_bytes.saturating_add(rollout_bytes)
    }

    fn rollout_record(&self) -> Option<FileRecord> {
        self.rollout_signature
            .clone()
            .map(|signature| FileRecord { signature })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BackupWorkTotals {
    items: usize,
    bytes: u64,
}

#[derive(Debug)]
struct BackupProgress {
    started_at: Instant,
    totals: BackupWorkTotals,
    state: Mutex<BackupProgressState>,
}

#[derive(Debug, Default)]
struct BackupProgressState {
    started_items: usize,
    completed_items: usize,
    completed_bytes: u64,
    in_flight_bytes: BTreeMap<usize, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProgressEstimates {
    bytes_remaining: u64,
    bytes_per_second: Option<u64>,
    average_time_per_item: Option<Duration>,
    eta_from_items: Option<Duration>,
    eta_from_bytes: Option<Duration>,
}

impl BackupProgress {
    fn new(totals: BackupWorkTotals) -> Self {
        Self {
            started_at: Instant::now(),
            totals,
            state: Mutex::new(BackupProgressState::default()),
        }
    }

    fn observed_bytes(state: &BackupProgressState) -> u64 {
        state
            .in_flight_bytes
            .values()
            .fold(state.completed_bytes, |total, bytes| {
                total.saturating_add(*bytes)
            })
    }

    fn estimates(&self, state: &BackupProgressState) -> ProgressEstimates {
        progress_estimates(
            self.started_at.elapsed(),
            state.completed_items,
            self.totals.items,
            Self::observed_bytes(state),
            self.totals.bytes,
        )
    }

    fn start_item(&self) -> usize {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.started_items = state.started_items.saturating_add(1);
        let item = state.started_items;
        state.in_flight_bytes.insert(item, 0);
        item
    }

    fn log_file_progress(
        &self,
        item: usize,
        source: &Path,
        current_item_bytes: u64,
        item_started_at: Instant,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.in_flight_bytes.insert(item, current_item_bytes);
        let observed_bytes = Self::observed_bytes(&state);
        let estimates = self.estimates(&state);
        info!(
            source = %source.display(),
            items_completed = state.completed_items,
            items_total = self.totals.items,
            bytes_copied = %format_bytes(observed_bytes),
            bytes_total = %format_bytes(self.totals.bytes),
            bytes_remaining = %format_bytes(estimates.bytes_remaining),
            bytes_per_second = %format_optional_rate(estimates.bytes_per_second),
            current_item_time = %format_duration(item_started_at.elapsed()),
            average_time_per_item = %format_optional_duration(estimates.average_time_per_item),
            eta_from_items = %format_optional_duration(estimates.eta_from_items),
            eta_from_bytes = %format_optional_duration(estimates.eta_from_bytes),
            "Backup file copy progress"
        );
    }

    fn complete_item(&self, item: usize, item_started_at: Instant, bytes_copied: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.completed_items = state.completed_items.saturating_add(1);
        state.completed_bytes = state.completed_bytes.saturating_add(bytes_copied);
        state.in_flight_bytes.remove(&item);
        let estimates = self.estimates(&state);
        info!(
            items_completed = state.completed_items,
            items_total = self.totals.items,
            last_item_time = %format_duration(item_started_at.elapsed()),
            average_time_per_item = %format_optional_duration(estimates.average_time_per_item),
            eta_from_items = %format_optional_duration(estimates.eta_from_items),
            bytes_copied = %format_bytes(Self::observed_bytes(&state)),
            bytes_total = %format_bytes(self.totals.bytes),
            bytes_remaining = %format_bytes(estimates.bytes_remaining),
            bytes_per_second = %format_optional_rate(estimates.bytes_per_second),
            eta_from_bytes = %format_optional_duration(estimates.eta_from_bytes),
            "Backup item complete"
        );
    }

    fn complete_unitemized_bytes(&self, bytes_copied: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.completed_bytes = state.completed_bytes.saturating_add(bytes_copied);
    }

    fn log_snapshot(&self, sessions_processed: usize, sessions_total: usize) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let estimates = self.estimates(&state);
        info!(
            sessions_processed,
            sessions_total,
            items_completed = state.completed_items,
            items_total = self.totals.items,
            average_time_per_item = %format_optional_duration(estimates.average_time_per_item),
            eta_from_items = %format_optional_duration(estimates.eta_from_items),
            bytes_copied = %format_bytes(Self::observed_bytes(&state)),
            bytes_total = %format_bytes(self.totals.bytes),
            bytes_remaining = %format_bytes(estimates.bytes_remaining),
            bytes_per_second = %format_optional_rate(estimates.bytes_per_second),
            eta_from_bytes = %format_optional_duration(estimates.eta_from_bytes),
            "Backup progress"
        );
    }
}

pub fn default_backup_directory() -> PathBuf {
    PathBuf::from(DEFAULT_BACKUP_DIRECTORY)
}

pub fn default_backup_jobs() -> usize {
    thread::available_parallelism().map_or(1, |parallelism| parallelism.get().min(4))
}

pub fn configured_backup_directory() -> eyre::Result<PathBuf> {
    let config_path = backup_config_path()?;
    Ok(load_destination(&config_path)?.0)
}

pub fn directory_info(source: &Path) -> eyre::Result<BackupDirectoryInfo> {
    let config_path = backup_config_path()?;
    let (destination, configured) = load_destination(&config_path)?;
    Ok(BackupDirectoryInfo {
        source: display_path(source),
        destination: display_path(&destination),
        config_path: display_path(&config_path),
        configured,
    })
}

pub fn set_backup_directory(path: &Path) -> eyre::Result<BackupDirectoryInfo> {
    let destination = absolute_path(path)?;
    let config_path = backup_config_path()?;
    let config_directory = config_path
        .parent()
        .context("backup configuration path has no parent directory")?;
    fs::create_dir_all(config_directory).with_context(|| {
        format!(
            "failed to create backup configuration directory {}",
            config_directory.display()
        )
    })?;
    write_json_atomic(
        &config_path,
        &BackupConfig {
            destination: display_path(&destination),
        },
    )?;
    Ok(BackupDirectoryInfo {
        source: String::new(),
        destination: display_path(&destination),
        config_path: display_path(&config_path),
        configured: true,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "the backup transaction keeps manifest updates and source enumeration together"
)]
pub fn run_backup(
    source: &Path,
    destination: &Path,
    cancellation_token: &CancellationToken,
    parallel_jobs: usize,
) -> eyre::Result<BackupNowReport> {
    if !(1..=MAX_BACKUP_JOBS).contains(&parallel_jobs) {
        bail!("--jobs must be between 1 and {MAX_BACKUP_JOBS}");
    }
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve Codex home {}", source.display()))?;
    let destination = absolute_path(destination)?;
    validate_backup_paths(&source, &destination)?;
    fs::create_dir_all(&destination).with_context(|| {
        format!(
            "failed to create backup destination {}",
            destination.display()
        )
    })?;

    let manifest_path = destination.join(BACKUP_MANIFEST_FILE);
    let previous_manifest =
        load_manifest(&manifest_path)?.filter(|manifest| manifest.source == display_path(&source));
    let mut manifest = previous_manifest.unwrap_or_else(|| BackupManifest {
        format_version: BACKUP_MANIFEST_VERSION,
        source: display_path(&source),
        completed_at: String::new(),
        sessions: BTreeMap::new(),
        global_files: BTreeMap::new(),
    });
    manifest.format_version = BACKUP_MANIFEST_VERSION;
    manifest.source = display_path(&source);

    let threads = all_threads(&source)?;
    let session_work = threads
        .iter()
        .map(|thread| plan_session_backup(&source, &destination, thread, Some(&manifest)))
        .collect::<eyre::Result<Vec<_>>>()?;
    let global_sources = collect_global_sources(&source)?;
    let totals = backup_work_totals(&session_work, &global_sources, &destination, &manifest);
    let progress = BackupProgress::new(totals);
    info!(
        source = %source.display(),
        destination = %destination.display(),
        parallel_jobs,
        sessions_total = threads.len(),
        items_total = totals.items,
        bytes_total = %format_bytes(totals.bytes),
        bytes_remaining = %format_bytes(totals.bytes),
        "Backup started"
    );
    let mut seen_sessions = BTreeSet::new();
    let mut sessions_copied = 0;
    let mut sessions_skipped = 0;
    let mut bytes_copied = 0_u64;
    let mut active_sessions = 0;
    let mut archived_sessions = 0;

    let session_results =
        execute_session_backups(&session_work, parallel_jobs, cancellation_token, &progress)?;
    for result in session_results {
        let work = &session_work[result.work_index];
        if result.wrote_files {
            sessions_copied += 1;
            bytes_copied = bytes_copied.saturating_add(result.bytes_copied);
        } else {
            sessions_skipped += 1;
        }
        if work.archived {
            archived_sessions += 1;
        } else {
            active_sessions += 1;
        }
        manifest.sessions.insert(
            work.session_id.clone(),
            SessionRecord {
                present: true,
                archived: work.archived,
                rollout: work.rollout_record(),
            },
        );
        seen_sessions.insert(work.session_id.clone());
    }

    for (session_id, record) in &mut manifest.sessions {
        if !seen_sessions.contains(session_id) {
            record.present = false;
        }
    }

    info!(
        global_files_total = global_sources.len(),
        "Session backup complete; backing up global files"
    );
    let mut seen_global_files = BTreeSet::new();
    let mut global_files_copied = 0;
    let mut global_files_skipped = 0;
    for global_source in &global_sources {
        cancellation_token.bail_if_cancelled()?;
        let destination_file = destination.join(&global_source.relative);
        if global_file_is_current(&manifest, global_source, &destination_file) {
            global_files_skipped += 1;
        } else {
            let item_started_at = Instant::now();
            let item = progress.start_item();
            info!(
                item,
                items_total = progress.totals.items,
                file = global_source.relative,
                size = %format_bytes(global_source.signature.size),
                "Copying global file"
            );
            let copied = copy_file_atomic(
                &global_source.source,
                &destination_file,
                cancellation_token,
                &progress,
                item,
                item_started_at,
            )?;
            bytes_copied = bytes_copied.saturating_add(copied);
            global_files_copied += 1;
            progress.complete_item(item, item_started_at, copied);
        }
        manifest.global_files.insert(
            global_source.relative.clone(),
            GlobalFileRecord {
                present: true,
                signature: file_signature(&global_source.source)?,
            },
        );
        seen_global_files.insert(global_source.relative.clone());
    }
    for (relative, record) in &mut manifest.global_files {
        if !seen_global_files.contains(relative) {
            record.present = false;
        }
    }

    manifest.completed_at = Utc::now().to_rfc3339();
    write_json_atomic(&manifest_path, &manifest)?;
    let removed_sessions_retained = manifest
        .sessions
        .values()
        .filter(|record| !record.present)
        .count();
    let removed_global_files_retained = manifest
        .global_files
        .values()
        .filter(|record| !record.present)
        .count();

    info!(
        sessions_copied,
        sessions_skipped,
        global_files_copied,
        global_files_skipped,
        bytes_copied = %format_bytes(bytes_copied),
        elapsed = %format_duration(progress.started_at.elapsed()),
        manifest = %manifest_path.display(),
        "Backup complete"
    );

    Ok(BackupNowReport {
        source: display_path(&source),
        destination: display_path(&destination),
        parallel_jobs,
        sessions_scanned: threads.len(),
        active_sessions,
        archived_sessions,
        sessions_copied,
        sessions_skipped,
        global_files_scanned: global_sources.len(),
        global_files_copied,
        global_files_skipped,
        bytes_copied,
        removed_sessions_retained,
        removed_global_files_retained,
        manifest_path: display_path(&manifest_path),
        completed_at: manifest.completed_at,
    })
}

fn execute_session_backups(
    work: &[SessionBackupWork],
    parallel_jobs: usize,
    cancellation_token: &CancellationToken,
    progress: &BackupProgress,
) -> eyre::Result<Vec<SessionBackupResult>> {
    if work.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallel_jobs.min(work.len());
    let next_index = AtomicUsize::new(0);
    let processed_sessions = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(work.len()));
    let first_error = Mutex::new(None::<eyre::Report>);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    if cancellation_token.is_cancelled() {
                        break;
                    }
                    let work_index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(session_work) = work.get(work_index) else {
                        break;
                    };
                    match execute_session_backup(
                        work_index,
                        work.len(),
                        session_work,
                        cancellation_token,
                        progress,
                    ) {
                        Ok(result) => {
                            results
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(result);
                            let processed = processed_sessions.fetch_add(1, Ordering::Relaxed) + 1;
                            if processed.is_multiple_of(50) || processed == work.len() {
                                progress.log_snapshot(processed, work.len());
                            }
                        }
                        Err(error) => {
                            let mut first_error = first_error
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if first_error.is_none() {
                                if !cancellation_token.is_cancelled() {
                                    cancellation_token
                                        .request_cancel("Parallel backup worker failed");
                                }
                                *first_error = Some(error);
                            }
                            break;
                        }
                    }
                }
            });
        }
    });

    if let Some(error) = first_error
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(error);
    }
    cancellation_token.bail_if_cancelled()?;
    let mut results = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if results.len() != work.len() {
        bail!(
            "parallel backup completed {} of {} planned sessions",
            results.len(),
            work.len()
        );
    }
    results.sort_by_key(|result| result.work_index);
    Ok(results)
}

fn execute_session_backup(
    work_index: usize,
    sessions_total: usize,
    work: &SessionBackupWork,
    cancellation_token: &CancellationToken,
    progress: &BackupProgress,
) -> eyre::Result<SessionBackupResult> {
    cancellation_token.bail_if_cancelled()?;
    let item_started_at = work.rollout_needs_copy.then(Instant::now);
    let item = work.rollout_needs_copy.then(|| progress.start_item());
    let mut bytes_copied = 0_u64;
    if work.needs_write() {
        let session_directory = work
            .metadata_path
            .parent()
            .context("session metadata path has no parent directory")?;
        fs::create_dir_all(session_directory).with_context(|| {
            format!(
                "failed to create normalized session directory {}",
                session_directory.display()
            )
        })?;
    }
    if work.rollout_needs_copy {
        let rollout_source = work
            .rollout_source
            .as_ref()
            .context("planned rollout copy has no source")?;
        let signature = work
            .rollout_signature
            .as_ref()
            .context("planned rollout copy has no signature")?;
        let item = item.context("planned rollout copy has no progress item")?;
        info!(
            session = work_index + 1,
            sessions_total,
            item,
            items_total = progress.totals.items,
            session_id = work.session_id,
            size = %format_bytes(signature.size),
            "Copying session rollout"
        );
        bytes_copied = bytes_copied.saturating_add(copy_file_atomic(
            rollout_source,
            &work.rollout_destination,
            cancellation_token,
            progress,
            item,
            item_started_at.context("planned rollout copy has no start time")?,
        )?);
    }
    if work.metadata_changed {
        write_bytes_atomic(&work.metadata_path, &work.metadata_bytes)?;
        bytes_copied = bytes_copied
            .saturating_add(u64::try_from(work.metadata_bytes.len()).unwrap_or(u64::MAX));
    }
    if let (Some(item), Some(item_started_at)) = (item, item_started_at) {
        progress.complete_item(item, item_started_at, bytes_copied);
    } else if bytes_copied > 0 {
        progress.complete_unitemized_bytes(bytes_copied);
    }
    Ok(SessionBackupResult {
        work_index,
        wrote_files: work.needs_write(),
        bytes_copied,
    })
}

pub fn status(source: &Path, destination: &Path) -> eyre::Result<BackupStatus> {
    let source = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve Codex home {}", source.display()))?;
    let destination = absolute_path(destination)?;
    validate_backup_paths(&source, &destination)?;
    let config_path = backup_config_path()?;
    let manifest_path = destination.join(BACKUP_MANIFEST_FILE);
    let manifest = load_manifest(&manifest_path)?;
    let manifest_source_matches = manifest
        .as_ref()
        .is_some_and(|manifest| manifest.source == display_path(&source));
    let threads = all_threads(&source)?;
    let global_sources = collect_global_sources(&source)?;
    let mut source_session_ids = BTreeSet::new();
    let mut pending_sessions = 0;
    let mut pending_session_bytes = 0_u64;
    let mut active_sessions = 0;
    let mut archived_sessions = 0;
    let current_manifest = manifest.as_ref().filter(|_| manifest_source_matches);

    for thread in &threads {
        let work = plan_session_backup(&source, &destination, thread, current_manifest)?;
        source_session_ids.insert(work.session_id.clone());
        if thread.archived {
            archived_sessions += 1;
        } else {
            active_sessions += 1;
        }
        if work.needs_write() {
            pending_sessions += 1;
            pending_session_bytes = pending_session_bytes.saturating_add(work.planned_bytes());
        }
    }

    let mut pending_global_files = 0;
    let mut pending_global_bytes = 0;
    let mut source_global_paths = BTreeSet::new();
    for global_source in &global_sources {
        source_global_paths.insert(global_source.relative.clone());
        let destination_file = destination.join(&global_source.relative);
        let is_current = current_manifest.is_some_and(|manifest| {
            global_file_is_current(manifest, global_source, &destination_file)
        });
        if !is_current {
            pending_global_files += 1;
            pending_global_bytes += global_source.signature.size;
        }
    }

    let tracked_sessions = manifest.as_ref().map_or(0, |manifest| {
        manifest
            .sessions
            .values()
            .filter(|record| record.present)
            .count()
    });
    let tracked_global_files = manifest.as_ref().map_or(0, |manifest| {
        manifest
            .global_files
            .values()
            .filter(|record| record.present)
            .count()
    });
    let removed_sessions_retained = manifest.as_ref().map_or(0, |manifest| {
        manifest
            .sessions
            .iter()
            .filter(|(session_id, record)| {
                record.present && !source_session_ids.contains(*session_id)
            })
            .count()
    });
    let removed_global_files_retained = manifest.as_ref().map_or(0, |manifest| {
        manifest
            .global_files
            .iter()
            .filter(|(relative, record)| record.present && !source_global_paths.contains(*relative))
            .count()
    });

    Ok(BackupStatus {
        source: display_path(&source),
        destination: display_path(&destination),
        config_path: display_path(&config_path),
        manifest_path: display_path(&manifest_path),
        destination_exists: destination.is_dir(),
        manifest_exists: manifest.is_some(),
        manifest_source_matches,
        last_backup_at: manifest.map(|manifest| manifest.completed_at),
        source_sessions: threads.len(),
        active_sessions,
        archived_sessions,
        tracked_sessions,
        pending_sessions,
        pending_session_bytes,
        source_global_files: global_sources.len(),
        tracked_global_files,
        pending_global_files,
        pending_global_bytes,
        removed_sessions_retained,
        removed_global_files_retained,
    })
}

fn all_threads(source: &Path) -> eyre::Result<Vec<ThreadSummary>> {
    list_threads(
        source,
        &ThreadListArgs {
            limit: Some(BACKUP_THREAD_LIMIT),
            all: true,
            archived: false,
        },
    )
}

fn plan_session_backup(
    source: &Path,
    destination: &Path,
    thread: &ThreadSummary,
    manifest: Option<&BackupManifest>,
) -> eyre::Result<SessionBackupWork> {
    let session_id = safe_session_id(&thread.id)?;
    let session_directory = destination.join("sessions").join(&session_id);
    let metadata_path = session_directory.join("metadata.json");
    let rollout_destination = session_directory.join("rollout.jsonl");
    let rollout_source = resolve_rollout_source(source, &thread.rollout_path)?;
    let rollout_signature = rollout_source.as_deref().map(file_signature).transpose()?;
    let metadata = session_metadata(thread, rollout_signature.clone());
    let metadata_bytes = facet_json::to_vec_pretty(&metadata)?;
    let metadata_changed =
        fs::read(&metadata_path).map_or(true, |existing| existing != metadata_bytes);
    let rollout_needs_copy = rollout_signature.as_ref().is_some_and(|signature| {
        !session_rollout_is_current(
            manifest.and_then(|value| value.sessions.get(&session_id)),
            signature,
            &rollout_destination,
            !metadata_changed,
        )
    });
    Ok(SessionBackupWork {
        session_id,
        archived: thread.archived,
        metadata_path,
        metadata_bytes,
        metadata_changed,
        rollout_source,
        rollout_destination,
        rollout_signature,
        rollout_needs_copy,
    })
}

fn session_rollout_is_current(
    manifest_record: Option<&SessionRecord>,
    signature: &FileSignature,
    destination: &Path,
    checkpoint_metadata_current: bool,
) -> bool {
    if !destination_matches_signature(destination, signature) {
        return false;
    }
    let manifest_current = manifest_record.is_some_and(|record| {
        record.present
            && record
                .rollout
                .as_ref()
                .is_some_and(|rollout| rollout.signature == *signature)
    });
    manifest_current || checkpoint_metadata_current
}

fn destination_matches_signature(destination: &Path, signature: &FileSignature) -> bool {
    fs::metadata(destination)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == signature.size)
}

fn backup_work_totals(
    session_work: &[SessionBackupWork],
    global_sources: &[GlobalSourceFile],
    destination: &Path,
    manifest: &BackupManifest,
) -> BackupWorkTotals {
    let mut totals = BackupWorkTotals::default();
    for work in session_work.iter().filter(|work| work.needs_write()) {
        if work.rollout_needs_copy {
            totals.items = totals.items.saturating_add(1);
        }
        totals.bytes = totals.bytes.saturating_add(work.planned_bytes());
    }
    for global_source in global_sources {
        let destination_file = destination.join(&global_source.relative);
        if !global_file_is_current(manifest, global_source, &destination_file) {
            totals.items = totals.items.saturating_add(1);
            totals.bytes = totals.bytes.saturating_add(global_source.signature.size);
        }
    }
    totals
}

fn global_file_is_current(
    manifest: &BackupManifest,
    source: &GlobalSourceFile,
    destination: &Path,
) -> bool {
    manifest
        .global_files
        .get(&source.relative)
        .is_some_and(|record| {
            record.present
                && record.signature == source.signature
                && destination_matches_signature(destination, &source.signature)
        })
}

fn session_metadata(
    thread: &ThreadSummary,
    rollout_source_signature: Option<FileSignature>,
) -> BackupSessionMetadata {
    BackupSessionMetadata {
        id: thread.id.clone(),
        title: thread.title.clone(),
        cwd: thread.cwd.clone(),
        created_at_ms: thread.created_at_ms,
        updated_at_ms: thread.updated_at_ms,
        recency_at_ms: thread.recency_at_ms,
        archived: thread.archived,
        tokens_used: thread.tokens_used,
        source: thread.source.clone(),
        thread_source: thread.thread_source.clone(),
        model: thread.model.clone(),
        reasoning_effort: thread.reasoning_effort.clone(),
        rollout_file: rollout_source_signature
            .as_ref()
            .map(|_| "rollout.jsonl".to_owned()),
        rollout_source_signature,
    }
}

fn resolve_rollout_source(source: &Path, raw_path: &str) -> eyre::Result<Option<PathBuf>> {
    let path = PathBuf::from(raw_path.strip_prefix(r"\\?\").unwrap_or(raw_path));
    if !path.is_file() {
        return Ok(None);
    }
    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("failed to resolve rollout path {}", path.display()))?;
    if !canonical.starts_with(source) {
        bail!(
            "rollout path {} is outside Codex home {}",
            canonical.display(),
            source.display()
        );
    }
    Ok(Some(canonical))
}

fn collect_global_sources(source: &Path) -> eyre::Result<Vec<GlobalSourceFile>> {
    let mut files = Vec::new();
    for relative in GLOBAL_FILES {
        let path = source.join(relative);
        if path.is_file() {
            files.push(GlobalSourceFile {
                signature: file_signature(&path)?,
                source: path,
                relative: format!("global/{relative}"),
            });
        }
    }
    for directory in GLOBAL_DIRECTORIES {
        let path = source.join(directory);
        if path.is_dir() {
            collect_global_directory(source, &path, &mut files)?;
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

fn collect_global_directory(
    source: &Path,
    directory: &Path,
    files: &mut Vec<GlobalSourceFile>,
) -> eyre::Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to enumerate {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_global_directory(source, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(source)
                .with_context(|| format!("{} is outside {}", path.display(), source.display()))
                .map(relative_path_string)?;
            files.push(GlobalSourceFile {
                signature: file_signature(&path)?,
                source: path,
                relative: format!("global/{relative}"),
            });
        }
    }
    Ok(())
}

fn file_signature(path: &Path) -> eyre::Result<FileSignature> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect source file {}", path.display()))?;
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    Ok(FileSignature {
        size: metadata.len(),
        modified_unix_ms,
    })
}

fn copy_file_atomic(
    source: &Path,
    destination: &Path,
    cancellation_token: &CancellationToken,
    progress: &BackupProgress,
    item: usize,
    item_started_at: Instant,
) -> eyre::Result<u64> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temporary = temporary_path(destination);
    let result = (|| -> eyre::Result<()> {
        let mut input = File::open(source)
            .with_context(|| format!("failed to open source file {}", source.display()))?;
        let mut output = File::create(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        let total_bytes = input.metadata()?.len();
        let mut copied_bytes = 0_u64;
        let mut next_progress_bytes = COPY_PROGRESS_BYTES;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            cancellation_token.bail_if_cancelled()?;
            let read_bytes = input
                .read(&mut buffer)
                .with_context(|| format!("failed to read {}", source.display()))?;
            if read_bytes == 0 {
                break;
            }
            output
                .write_all(&buffer[..read_bytes])
                .with_context(|| format!("failed to copy {}", source.display()))?;
            copied_bytes = copied_bytes.saturating_add(
                u64::try_from(read_bytes).expect("copy buffer length always fits in u64"),
            );
            if copied_bytes >= next_progress_bytes && copied_bytes < total_bytes {
                progress.log_file_progress(item, source, copied_bytes, item_started_at);
                next_progress_bytes = next_progress_bytes.saturating_add(COPY_PROGRESS_BYTES);
            }
        }
        cancellation_token.bail_if_cancelled()?;
        output.sync_all().with_context(|| {
            format!(
                "failed to flush temporary backup file {}",
                temporary.display()
            )
        })?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    replace_file(&temporary, destination)?;
    Ok(fs::metadata(destination)?.len())
}

fn format_bytes(bytes: u64) -> String {
    const KIBIBYTE: u64 = 1024;
    const MEBIBYTE: u64 = 1024 * KIBIBYTE;
    const GIBIBYTE: u64 = 1024 * MEBIBYTE;

    let (unit_bytes, unit) = if bytes >= GIBIBYTE {
        (GIBIBYTE, "GiB")
    } else if bytes >= MEBIBYTE {
        (MEBIBYTE, "MiB")
    } else if bytes >= KIBIBYTE {
        (KIBIBYTE, "KiB")
    } else {
        return format!("{bytes} B");
    };
    let whole = bytes / unit_bytes;
    let hundredths = (bytes % unit_bytes) * 100 / unit_bytes;
    format!("{whole}.{hundredths:02} {unit}")
}

fn progress_estimates(
    elapsed: Duration,
    completed_items: usize,
    total_items: usize,
    observed_bytes: u64,
    total_bytes: u64,
) -> ProgressEstimates {
    let elapsed_ms = elapsed.as_millis().max(1);
    let bytes_remaining = total_bytes.saturating_sub(observed_bytes);
    let bytes_per_second = (observed_bytes > 0).then(|| {
        let rate = u128::from(observed_bytes).saturating_mul(1000) / elapsed_ms;
        u64::try_from(rate).unwrap_or(u64::MAX)
    });
    let eta_from_bytes = bytes_per_second
        .filter(|rate| *rate > 0)
        .map(|rate| duration_from_millis(u128::from(bytes_remaining) * 1000 / u128::from(rate)));
    let average_time_per_item = (completed_items > 0).then(|| {
        duration_from_millis(elapsed_ms / u128::try_from(completed_items).unwrap_or(u128::MAX))
    });
    let remaining_items = total_items.saturating_sub(completed_items);
    let eta_from_items = average_time_per_item.map(|average| {
        duration_from_millis(
            average
                .as_millis()
                .saturating_mul(u128::try_from(remaining_items).unwrap_or(u128::MAX)),
        )
    });
    ProgressEstimates {
        bytes_remaining,
        bytes_per_second,
        average_time_per_item,
        eta_from_items,
        eta_from_bytes,
    }
}

fn duration_from_millis(milliseconds: u128) -> Duration {
    Duration::from_millis(u64::try_from(milliseconds).unwrap_or(u64::MAX))
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        return format!("{}ms", duration.as_millis());
    }
    let seconds = duration.as_secs();
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

fn format_optional_duration(duration: Option<Duration>) -> String {
    duration.map_or_else(|| "calculating".to_owned(), format_duration)
}

fn format_rate(bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_second))
}

fn format_optional_rate(bytes_per_second: Option<u64>) -> String {
    bytes_per_second.map_or_else(|| "calculating".to_owned(), format_rate)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> eyre::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let temporary = temporary_path(path);
    let result = (|| -> eyre::Result<()> {
        let mut file = File::create(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", temporary.display()))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    replace_file(&temporary, path)
}

fn write_json_atomic<T: Facet<'static>>(path: &Path, value: &T) -> eyre::Result<()> {
    let bytes = facet_json::to_vec_pretty(value).wrap_err("failed to serialize JSON")?;
    write_bytes_atomic(path, &bytes)
}

fn load_manifest(path: &Path) -> eyre::Result<Option<BackupManifest>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: BackupManifest = facet_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if manifest.format_version != BACKUP_MANIFEST_VERSION {
        bail!(
            "unsupported backup manifest version {} in {}",
            manifest.format_version,
            path.display()
        );
    }
    Ok(Some(manifest))
}

fn backup_config_path() -> eyre::Result<PathBuf> {
    let config_root = dirs::config_dir()
        .or_else(dirs::home_dir)
        .context("could not determine a configuration directory")?;
    Ok(config_root
        .join(BACKUP_CONFIG_DIRECTORY)
        .join(BACKUP_CONFIG_FILE))
}

fn load_destination(config_path: &Path) -> eyre::Result<(PathBuf, bool)> {
    if !config_path.is_file() {
        return Ok((default_backup_directory(), false));
    }
    let text = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config: BackupConfig = facet_json::from_str(&text)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    if config.destination.trim().is_empty() {
        bail!(
            "configured backup destination is empty in {}",
            config_path.display()
        );
    }
    Ok((absolute_path(Path::new(&config.destination))?, true))
}

fn replace_file(temporary: &Path, destination: &Path) -> eyre::Result<()> {
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("failed to replace {}", destination.display()))?;
    }
    fs::rename(temporary, destination).with_context(|| {
        format!(
            "failed to move temporary backup {} to {}",
            temporary.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn validate_backup_paths(source: &Path, destination: &Path) -> eyre::Result<()> {
    if source == destination || destination.starts_with(source) {
        bail!(
            "backup destination {} must not be the Codex home or one of its descendants {}",
            destination.display(),
            source.display()
        );
    }
    Ok(())
}

fn absolute_path(path: &Path) -> eyre::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let file_name = path.file_name().map_or_else(
        || "backup".to_owned(),
        |file_name| file_name.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(
        ".{file_name}.teamy-codex-{}-{suffix}.tmp",
        std::process::id()
    ))
}

fn safe_session_id(id: &str) -> eyre::Result<String> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id
            .chars()
            .any(|character| character == '\\' || character == '/' || character == ':')
    {
        bail!("Codex session id is not safe as a backup directory name: {id:?}");
    }
    Ok(id.to_owned())
}

fn relative_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn parallel_normalized_backup_is_incremental_and_resumable() {
        let root = std::env::temp_dir().join(format!(
            "teamy-codex-normalized-backup-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after the Unix epoch")
                .as_nanos()
        ));
        let source = root.join("source");
        let destination = root.join("destination");
        let rollout = source
            .join("sessions")
            .join("2026")
            .join("01")
            .join("rollout.jsonl");
        fs::create_dir_all(rollout.parent().expect("rollout should have a parent"))
            .expect("source directories should be created");
        fs::write(&rollout, b"first rollout\n").expect("rollout should be written");
        let second_rollout = rollout.with_file_name("second-rollout.jsonl");
        fs::write(&second_rollout, b"second rollout\n").expect("second rollout should be written");
        let database = source.join("state_5.sqlite");
        let connection = Connection::open(&database).expect("state database should be created");
        connection
            .execute_batch(
                "create table threads (
                    id text not null,
                    title text not null,
                    cwd text not null,
                    created_at_ms integer,
                    updated_at_ms integer,
                    recency_at_ms integer not null,
                    archived integer not null,
                    tokens_used integer not null,
                    source text not null,
                    thread_source text,
                    model text,
                    reasoning_effort text,
                    rollout_path text not null,
                    updated_at integer
                );",
            )
            .expect("state schema should be created");
        connection
            .execute(
                "insert into threads values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                rusqlite::params![
                    "session-123",
                    "Example session",
                    "C:\\work",
                    1_i64,
                    2_i64,
                    3_i64,
                    0_i64,
                    4_i64,
                    "cli",
                    Option::<String>::None,
                    Some("model"),
                    Some("medium"),
                    rollout.to_string_lossy(),
                    2_i64,
                ],
            )
            .expect("thread should be inserted");
        connection
            .execute(
                "insert into threads values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                rusqlite::params![
                    "session-456",
                    "Second session",
                    "C:\\work",
                    1_i64,
                    2_i64,
                    3_i64,
                    1_i64,
                    4_i64,
                    "cli",
                    Option::<String>::None,
                    Some("model"),
                    Some("medium"),
                    second_rollout.to_string_lossy(),
                    2_i64,
                ],
            )
            .expect("second thread should be inserted");
        drop(connection);
        let cancellation_token = CancellationToken::new();

        let first = run_backup(&source, &destination, &cancellation_token, 2)
            .expect("first backup should succeed");
        assert_eq!(first.parallel_jobs, 2);
        assert_eq!(first.sessions_copied, 2);
        assert!(
            destination
                .join("sessions/session-123/metadata.json")
                .is_file()
        );
        assert!(
            destination
                .join("sessions/session-123/rollout.jsonl")
                .is_file()
        );
        assert!(
            destination
                .join("sessions/session-456/rollout.jsonl")
                .is_file()
        );

        fs::remove_file(destination.join(BACKUP_MANIFEST_FILE))
            .expect("completed manifest should be removable for resume simulation");
        let checkpoint_status = status(&source, &destination)
            .expect("per-session checkpoint should support status without a final manifest");
        assert_eq!(checkpoint_status.pending_sessions, 0);

        let second = run_backup(&source, &destination, &cancellation_token, 2)
            .expect("backup should resume from the per-session checkpoint");
        assert_eq!(second.sessions_copied, 0);
        assert_eq!(second.sessions_skipped, 2);
        assert_eq!(second.bytes_copied, 0);

        fs::write(&rollout, b"changed rollout\n").expect("changed rollout should be written");
        let third = run_backup(&source, &destination, &cancellation_token, 2)
            .expect("changed backup should succeed");
        assert_eq!(third.sessions_copied, 1);
        assert_eq!(third.sessions_skipped, 1);
        assert_eq!(
            fs::read_to_string(destination.join("sessions/session-123/rollout.jsonl"))
                .expect("backup rollout should be readable"),
            "changed rollout\n"
        );

        fs::remove_dir_all(&root).expect("test directory should be removable");
    }

    #[test]
    fn progress_estimates_report_rate_and_both_eta_models() {
        let estimates = progress_estimates(Duration::from_secs(10), 2, 4, 500, 1_000);
        assert_eq!(estimates.bytes_remaining, 500);
        assert_eq!(estimates.bytes_per_second, Some(50));
        assert_eq!(
            estimates.average_time_per_item,
            Some(Duration::from_secs(5))
        );
        assert_eq!(estimates.eta_from_items, Some(Duration::from_secs(10)));
        assert_eq!(estimates.eta_from_bytes, Some(Duration::from_secs(10)));
    }

    #[test]
    fn parallel_progress_counts_every_in_flight_worker() {
        let mut state = BackupProgressState {
            completed_bytes: 100,
            ..BackupProgressState::default()
        };
        state.in_flight_bytes.insert(1, 200);
        state.in_flight_bytes.insert(2, 300);
        assert_eq!(BackupProgress::observed_bytes(&state), 600);
    }
}
