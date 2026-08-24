use chrono::DateTime;
use chrono::Utc;
use eyre::Context;
use eyre::ContextCompat;
use eyre::bail;
use facet::Facet;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::MoveFileExW;
#[cfg(windows)]
use windows::core::HSTRING;

const INVENTORY_MANIFEST_VERSION: u32 = 1;
const EPOCH_MILLISECONDS_THRESHOLD: i64 = 100_000_000_000;
const LAST_ACTIVITY_SEMANTICS: &str = "maximum normalized nonzero value across recency_at_ms, recency_at, updated_at_ms, updated_at, created_at_ms, and created_at; values below 100000000000 are interpreted as Unix seconds and multiplied by 1000";
const INVENTORY_ORDERING: &str = "last_activity_ms descending, then task_id ascending";
const CANONICAL_ROWS_FORMAT: &str =
    "UTF-8 facet-json compact JSON array in declared field order after inventory ordering";

#[derive(Debug)]
pub struct InventoryRequest {
    pub active_since: String,
    pub cwd_under: Vec<String>,
    pub include_archived: bool,
    pub limit: Option<usize>,
    pub manifest_path: PathBuf,
    pub overwrite_manifest: bool,
}

#[derive(Clone, Facet, Debug, Eq, PartialEq)]
pub struct InventoryRow {
    pub task_id: String,
    pub title: String,
    pub cwd: String,
    pub last_activity_ms: i64,
    pub last_activity_source: String,
    pub archived: bool,
    pub pinned: bool,
    pub agent_role: Option<String>,
    pub agent_path: Option<String>,
    pub agent_name: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub tokens_used: Option<i64>,
    pub source: Option<String>,
    pub thread_source: Option<String>,
}

#[derive(Facet, Debug)]
struct InventoryManifest {
    format_version: u32,
    kind: String,
    generated_at: String,
    generated_at_ms: i64,
    query: InventoryQueryManifest,
    sqlite: InventorySqliteManifest,
    counts: InventoryCountsManifest,
    rows: InventoryRowsManifest,
    tool: InventoryToolManifest,
}

#[derive(Facet, Debug)]
struct InventoryQueryManifest {
    active_since: String,
    active_since_source: String,
    active_since_duration_ms: i64,
    cutoff_ms: i64,
    cutoff_source: String,
    cutoff_inclusive: bool,
    cwd_under: Vec<InventoryCwdFilterManifest>,
    cwd_under_semantics: String,
    include_archived: bool,
    include_archived_source: String,
    limit: Option<u64>,
    limit_source: String,
    last_activity_semantics: String,
    ordering: String,
}

#[derive(Facet, Debug)]
struct InventoryCwdFilterManifest {
    input: String,
    normalized: String,
    source: String,
}

#[derive(Facet, Debug)]
struct InventorySqliteManifest {
    open_mode: String,
    sqlite_version: String,
    journal_mode: String,
    user_version: i64,
    schema_version: i64,
    data_version: i64,
    database_file: InventoryFileManifest,
    wal_file: InventoryFileManifest,
    shm_file: InventoryFileManifest,
    threads_schema_sql_sha256: String,
    threads_columns: Vec<InventorySqliteColumnManifest>,
}

#[derive(Facet, Debug)]
struct InventoryFileManifest {
    path: String,
    exists: bool,
    size_bytes: Option<u64>,
    modified_unix_ms: Option<i64>,
}

#[derive(Clone, Facet, Debug)]
struct InventorySqliteColumnManifest {
    ordinal: i64,
    name: String,
    declared_type: String,
    not_null: bool,
    primary_key: bool,
}

#[derive(Facet, Debug)]
struct InventoryCountsManifest {
    matched: u64,
    returned: u64,
    truncated: u64,
}

#[derive(Facet, Debug)]
struct InventoryRowsManifest {
    canonicalization: String,
    sha256: String,
}

#[derive(Facet, Debug)]
struct InventoryToolManifest {
    package_name: String,
    package_version: String,
    repository_url: String,
    repository_root: String,
    revision: String,
    branch: String,
    worktree_status: String,
    build_timestamp_unix: String,
    build_target: String,
    build_profile: String,
    rustc_version: String,
    executable: InventoryFileManifest,
}

#[derive(Debug)]
struct NormalizedCwdFilter {
    input: String,
    normalized: String,
    path: PathBuf,
}

#[derive(Debug)]
struct InventoryQuery<'a> {
    cutoff_ms: i64,
    cwd_under: &'a [NormalizedCwdFilter],
    include_archived: bool,
    limit: Option<usize>,
}

#[derive(Debug)]
struct InventoryQueryResult {
    rows: Vec<InventoryRow>,
    matched_count: usize,
    truncated_count: usize,
}

#[derive(Debug)]
struct RawInventoryRow {
    task_id: String,
    title: String,
    cwd: String,
    recency_at_ms: Option<i64>,
    recency_at: Option<i64>,
    updated_at_ms: Option<i64>,
    updated_at: Option<i64>,
    created_at_ms: Option<i64>,
    created_at: Option<i64>,
    archived: bool,
    pinned: bool,
    agent_role: Option<String>,
    agent_path: Option<String>,
    agent_name: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    tokens_used: Option<i64>,
    source: Option<String>,
    thread_source: Option<String>,
}

/// Capture a bounded, read-only task inventory and atomically write its audit manifest.
///
/// # Errors
///
/// Returns an error when arguments are invalid, the `SQLite` state cannot be read,
/// canonical rows cannot be hashed, or the manifest cannot be installed atomically.
pub fn capture_inventory(
    codex_home: &Path,
    request: &InventoryRequest,
) -> eyre::Result<Vec<InventoryRow>> {
    if request.active_since.trim().is_empty() {
        bail!("--active-since must not be empty");
    }
    if request.limit == Some(0) {
        bail!("--limit must be greater than zero when specified");
    }

    let manifest_path = absolute_path(&request.manifest_path)?;
    if manifest_path.exists() && !request.overwrite_manifest {
        bail!(
            "manifest {} already exists; pass --overwrite-manifest to replace it atomically",
            manifest_path.display()
        );
    }

    let active_since_duration = humantime::parse_duration(&request.active_since)
        .wrap_err_with(|| format!("invalid --active-since duration {:?}", request.active_since))?;
    let active_since_duration_ms = i64::try_from(active_since_duration.as_millis())
        .wrap_err("--active-since duration is too large")?;
    if active_since_duration_ms == 0 {
        bail!("--active-since must be at least one millisecond");
    }
    let generated_at_ms = Utc::now().timestamp_millis();
    let cutoff_ms = generated_at_ms
        .checked_sub(active_since_duration_ms)
        .context("--active-since produces a cutoff outside the supported timestamp range")?;
    let generated_at = DateTime::<Utc>::from_timestamp_millis(generated_at_ms)
        .context("capture timestamp is outside the supported range")?
        .to_rfc3339();
    let cwd_under = normalize_cwd_filters(&request.cwd_under)?;

    let database_path = codex_home.join("state_5.sqlite");
    let connection = open_state_database_read_only(&database_path)?;
    let columns = load_threads_columns(&connection)?;
    let query_result = query_inventory(
        &connection,
        &columns,
        &InventoryQuery {
            cutoff_ms,
            cwd_under: &cwd_under,
            include_archived: request.include_archived,
            limit: request.limit,
        },
    )?;
    let rows_sha256 = semantic_rows_sha256(&query_result.rows)?;
    let manifest = InventoryManifest {
        format_version: INVENTORY_MANIFEST_VERSION,
        kind: String::from("teamy-codex-thread-inventory"),
        generated_at,
        generated_at_ms,
        query: InventoryQueryManifest {
            active_since: request.active_since.clone(),
            active_since_source: String::from("command-line:--active-since"),
            active_since_duration_ms,
            cutoff_ms,
            cutoff_source: String::from(
                "generated_at_ms minus parsed command-line:--active-since duration",
            ),
            cutoff_inclusive: true,
            cwd_under: cwd_under
                .iter()
                .map(|filter| InventoryCwdFilterManifest {
                    input: filter.input.clone(),
                    normalized: filter.normalized.clone(),
                    source: String::from("command-line:--cwd-under"),
                })
                .collect(),
            cwd_under_semantics: String::from(
                "logical OR of lexical path-component containment; ASCII case-insensitive on Windows",
            ),
            include_archived: request.include_archived,
            include_archived_source: if request.include_archived {
                String::from("command-line:--include-archived")
            } else {
                String::from("default:false")
            },
            limit: request.limit.map(|limit| limit as u64),
            limit_source: if request.limit.is_some() {
                String::from("command-line:--limit")
            } else {
                String::from("default:none")
            },
            last_activity_semantics: String::from(LAST_ACTIVITY_SEMANTICS),
            ordering: String::from(INVENTORY_ORDERING),
        },
        sqlite: sqlite_manifest(&connection, &database_path, columns)?,
        counts: InventoryCountsManifest {
            matched: query_result.matched_count as u64,
            returned: query_result.rows.len() as u64,
            truncated: query_result.truncated_count as u64,
        },
        rows: InventoryRowsManifest {
            canonicalization: String::from(CANONICAL_ROWS_FORMAT),
            sha256: rows_sha256,
        },
        tool: tool_manifest()?,
    };
    let manifest_bytes = facet_json::to_vec_pretty(&manifest)
        .wrap_err("failed to serialize thread inventory manifest")?;
    write_manifest_atomic(&manifest_path, &manifest_bytes, request.overwrite_manifest)?;
    Ok(query_result.rows)
}

fn open_state_database_read_only(path: &Path) -> eyre::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .wrap_err_with(|| {
        format!(
            "failed to open Codex state database read-only: {}",
            path.display()
        )
    })?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .wrap_err("failed to enforce SQLite query-only mode")?;
    Ok(connection)
}

fn load_threads_columns(
    connection: &Connection,
) -> eyre::Result<Vec<InventorySqliteColumnManifest>> {
    let mut statement = connection.prepare("PRAGMA table_info(threads)")?;
    let columns = statement
        .query_map([], |row| {
            Ok(InventorySqliteColumnManifest {
                ordinal: row.get(0)?,
                name: row.get(1)?,
                declared_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if columns.is_empty() {
        bail!("Codex state database has no threads table");
    }
    Ok(columns)
}

fn query_inventory(
    connection: &Connection,
    columns: &[InventorySqliteColumnManifest],
    query: &InventoryQuery<'_>,
) -> eyre::Result<InventoryQueryResult> {
    let column_names = columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<BTreeSet<_>>();
    for required in ["id", "title", "cwd"] {
        if !column_names.contains(required) {
            bail!("threads table is missing required inventory column {required:?}");
        }
    }
    if ![
        "recency_at_ms",
        "recency_at",
        "updated_at_ms",
        "updated_at",
        "created_at_ms",
        "created_at",
    ]
    .iter()
    .any(|column| column_names.contains(column))
    {
        bail!("threads table has no supported activity timestamp columns");
    }

    let sql = format!(
        "SELECT threads.id, threads.title, threads.cwd, \
         {recency_at_ms} AS recency_at_ms, {recency_at} AS recency_at, \
         {updated_at_ms} AS updated_at_ms, {updated_at} AS updated_at, \
         {created_at_ms} AS created_at_ms, {created_at} AS created_at, \
         {archived} AS archived, {pinned} AS pinned, \
         {agent_role} AS agent_role, {agent_path} AS agent_path, \
         {agent_name} AS agent_name, {model} AS model, \
         {reasoning_effort} AS reasoning_effort, {tokens_used} AS tokens_used, \
         {source} AS source, {thread_source} AS thread_source FROM threads",
        recency_at_ms = optional_column(&column_names, "recency_at_ms", "NULL"),
        recency_at = optional_column(&column_names, "recency_at", "NULL"),
        updated_at_ms = optional_column(&column_names, "updated_at_ms", "NULL"),
        updated_at = optional_column(&column_names, "updated_at", "NULL"),
        created_at_ms = optional_column(&column_names, "created_at_ms", "NULL"),
        created_at = optional_column(&column_names, "created_at", "NULL"),
        archived = optional_column(&column_names, "archived", "0"),
        pinned = optional_column(&column_names, "is_pinned", "0"),
        agent_role = optional_column(&column_names, "agent_role", "NULL"),
        agent_path = optional_column(&column_names, "agent_path", "NULL"),
        agent_name = optional_column(&column_names, "agent_nickname", "NULL"),
        model = optional_column(&column_names, "model", "NULL"),
        reasoning_effort = optional_column(&column_names, "reasoning_effort", "NULL"),
        tokens_used = optional_column(&column_names, "tokens_used", "NULL"),
        source = optional_column(&column_names, "source", "NULL"),
        thread_source = optional_column(&column_names, "thread_source", "NULL"),
    );
    let mut statement = connection.prepare(&sql)?;
    let raw_rows = statement
        .query_map([], |row| {
            Ok(RawInventoryRow {
                task_id: row.get(0)?,
                title: row.get(1)?,
                cwd: row.get(2)?,
                recency_at_ms: row.get(3)?,
                recency_at: row.get(4)?,
                updated_at_ms: row.get(5)?,
                updated_at: row.get(6)?,
                created_at_ms: row.get(7)?,
                created_at: row.get(8)?,
                archived: row.get::<_, i64>(9)? != 0,
                pinned: row.get::<_, i64>(10)? != 0,
                agent_role: row.get(11)?,
                agent_path: row.get(12)?,
                agent_name: row.get(13)?,
                model: row.get(14)?,
                reasoning_effort: row.get(15)?,
                tokens_used: row.get(16)?,
                source: row.get(17)?,
                thread_source: row.get(18)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut rows = raw_rows
        .into_iter()
        .filter_map(|raw| inventory_row(raw, query))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .last_activity_ms
            .cmp(&left.last_activity_ms)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    let matched_count = rows.len();
    if let Some(limit) = query.limit {
        rows.truncate(limit);
    }
    let truncated_count = matched_count.saturating_sub(rows.len());
    Ok(InventoryQueryResult {
        rows,
        matched_count,
        truncated_count,
    })
}

fn optional_column(
    columns: &BTreeSet<&str>,
    name: &'static str,
    fallback: &'static str,
) -> &'static str {
    if columns.contains(name) {
        name
    } else {
        fallback
    }
}

fn inventory_row(raw: RawInventoryRow, query: &InventoryQuery<'_>) -> Option<InventoryRow> {
    if raw.archived && !query.include_archived {
        return None;
    }
    if !cwd_matches(&raw.cwd, query.cwd_under) {
        return None;
    }
    let (last_activity_ms, last_activity_source) = last_activity(&raw)?;
    if last_activity_ms < query.cutoff_ms {
        return None;
    }
    Some(InventoryRow {
        task_id: raw.task_id,
        title: raw.title,
        cwd: raw.cwd,
        last_activity_ms,
        last_activity_source: String::from(last_activity_source),
        archived: raw.archived,
        pinned: raw.pinned,
        agent_role: raw.agent_role,
        agent_path: raw.agent_path,
        agent_name: raw.agent_name,
        model: raw.model,
        reasoning_effort: raw.reasoning_effort,
        tokens_used: raw.tokens_used,
        source: raw.source,
        thread_source: raw.thread_source,
    })
}

fn last_activity(raw: &RawInventoryRow) -> Option<(i64, &'static str)> {
    [
        (raw.recency_at_ms, "recency_at_ms"),
        (raw.recency_at, "recency_at"),
        (raw.updated_at_ms, "updated_at_ms"),
        (raw.updated_at, "updated_at"),
        (raw.created_at_ms, "created_at_ms"),
        (raw.created_at, "created_at"),
    ]
    .into_iter()
    .filter_map(|(value, source)| normalize_epoch_milliseconds(value).map(|value| (value, source)))
    .max_by_key(|(value, _)| *value)
}

fn normalize_epoch_milliseconds(value: Option<i64>) -> Option<i64> {
    let value = value.filter(|value| *value > 0)?;
    if value < EPOCH_MILLISECONDS_THRESHOLD {
        value.checked_mul(1_000)
    } else {
        Some(value)
    }
}

fn normalize_cwd_filters(raw_filters: &[String]) -> eyre::Result<Vec<NormalizedCwdFilter>> {
    raw_filters
        .iter()
        .map(|input| {
            if input.trim().is_empty() {
                bail!("--cwd-under must not be empty");
            }
            let path = absolute_path(Path::new(normalize_extended_path(input)))?;
            Ok(NormalizedCwdFilter {
                input: input.clone(),
                normalized: display_path(&path),
                path,
            })
        })
        .collect()
}

fn cwd_matches(cwd: &str, filters: &[NormalizedCwdFilter]) -> bool {
    if filters.is_empty() {
        return true;
    }
    let cwd = PathBuf::from(normalize_extended_path(cwd));
    filters
        .iter()
        .any(|filter| path_starts_with(&cwd, &filter.path))
}

fn path_starts_with(path: &Path, base: &Path) -> bool {
    let mut path_components = path.components();
    base.components().all(|base_component| {
        path_components.next().is_some_and(|path_component| {
            component_eq(path_component.as_os_str(), base_component.as_os_str())
        })
    })
}

#[cfg(windows)]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left == right
}

fn semantic_rows_sha256(rows: &[InventoryRow]) -> eyre::Result<String> {
    let canonical = facet_json::to_vec(rows).wrap_err("failed to canonicalize inventory rows")?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn sqlite_manifest(
    connection: &Connection,
    database_path: &Path,
    columns: Vec<InventorySqliteColumnManifest>,
) -> eyre::Result<InventorySqliteManifest> {
    let sqlite_version = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let journal_mode = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let user_version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let schema_version = connection.query_row("PRAGMA schema_version", [], |row| row.get(0))?;
    let data_version = connection.query_row("PRAGMA data_version", [], |row| row.get(0))?;
    let threads_schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'threads'",
            [],
            |row| row.get(0),
        )
        .context("threads table has no sqlite_master schema entry")?;
    Ok(InventorySqliteManifest {
        open_mode: String::from("SQLITE_OPEN_READ_ONLY with PRAGMA query_only=ON"),
        sqlite_version,
        journal_mode,
        user_version,
        schema_version,
        data_version,
        database_file: file_manifest(database_path),
        wal_file: file_manifest(&sidecar_path(database_path, "-wal")),
        shm_file: file_manifest(&sidecar_path(database_path, "-shm")),
        threads_schema_sql_sha256: format!("{:x}", Sha256::digest(threads_schema.as_bytes())),
        threads_columns: columns,
    })
}

fn tool_manifest() -> eyre::Result<InventoryToolManifest> {
    let executable_path =
        std::env::current_exe().wrap_err("failed to resolve current executable")?;
    Ok(InventoryToolManifest {
        package_name: String::from(env!("CARGO_PKG_NAME")),
        package_version: String::from(env!("CARGO_PKG_VERSION")),
        repository_url: String::from(env!("GIT_REPOSITORY_URL")),
        repository_root: String::from(env!("CARGO_MANIFEST_DIR")),
        revision: String::from(env!("GIT_REVISION_FULL")),
        branch: String::from(env!("GIT_BRANCH")),
        worktree_status: String::from(env!("GIT_WORKTREE_STATUS_ALL")),
        build_timestamp_unix: String::from(env!("BUILD_TIMESTAMP_UNIX")),
        build_target: String::from(env!("BUILD_TARGET")),
        build_profile: String::from(env!("BUILD_PROFILE")),
        rustc_version: String::from(env!("RUSTC_VERSION")),
        executable: file_manifest(&executable_path),
    })
}

fn file_manifest(path: &Path) -> InventoryFileManifest {
    match fs::metadata(path) {
        Ok(metadata) => InventoryFileManifest {
            path: display_path(path),
            exists: true,
            size_bytes: Some(metadata.len()),
            modified_unix_ms: metadata.modified().ok().and_then(system_time_milliseconds),
        },
        Err(_) => InventoryFileManifest {
            path: display_path(path),
            exists: false,
            size_bytes: None,
            modified_unix_ms: None,
        },
    }
}

fn sidecar_path(database_path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", database_path.to_string_lossy()))
}

fn system_time_milliseconds(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn write_manifest_atomic(path: &Path, bytes: &[u8], overwrite: bool) -> eyre::Result<()> {
    let parent = path
        .parent()
        .context("manifest path has no parent directory")?;
    fs::create_dir_all(parent)
        .wrap_err_with(|| format!("failed to create manifest directory {}", parent.display()))?;
    let temporary = temporary_path(path);
    let write_result = (|| -> eyre::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .wrap_err_with(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(bytes)
            .wrap_err_with(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .wrap_err_with(|| format!("failed to flush {}", temporary.display()))?;
        install_manifest(&temporary, path, overwrite)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn install_manifest(temporary: &Path, destination: &Path, overwrite: bool) -> eyre::Result<()> {
    if overwrite {
        return replace_manifest(temporary, destination);
    }
    fs::hard_link(temporary, destination).wrap_err_with(|| {
        if destination.exists() {
            format!(
                "manifest {} already exists; pass --overwrite-manifest to replace it atomically",
                destination.display()
            )
        } else {
            format!(
                "failed to atomically install manifest {} without overwrite",
                destination.display()
            )
        }
    })?;
    fs::remove_file(temporary)
        .wrap_err_with(|| format!("failed to remove {}", temporary.display()))?;
    Ok(())
}

#[cfg(windows)]
fn replace_manifest(temporary: &Path, destination: &Path) -> eyre::Result<()> {
    // SAFETY: both paths are live, NUL-free HSTRING values and no pointers or
    // handles escape this synchronous call.
    unsafe {
        MoveFileExW(
            &HSTRING::from(temporary.to_string_lossy().into_owned()),
            &HSTRING::from(destination.to_string_lossy().into_owned()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .wrap_err_with(|| {
        format!(
            "failed to atomically replace manifest {}",
            destination.display()
        )
    })
}

#[cfg(not(windows))]
fn replace_manifest(temporary: &Path, destination: &Path) -> eyre::Result<()> {
    fs::rename(temporary, destination).wrap_err_with(|| {
        format!(
            "failed to atomically replace manifest {}",
            destination.display()
        )
    })
}

fn temporary_path(path: &Path) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let file_name = path.file_name().map_or_else(
        || String::from("inventory-manifest"),
        |file_name| file_name.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(
        ".{file_name}.teamy-codex-{}-{suffix}.tmp",
        std::process::id()
    ))
}

fn absolute_path(path: &Path) -> eyre::Result<PathBuf> {
    std::path::absolute(path)
        .wrap_err_with(|| format!("failed to make path absolute: {}", path.display()))
}

fn normalize_extended_path(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    const CUTOFF_MS: i64 = 1_700_000_000_000;

    #[derive(Debug)]
    struct FixtureTask<'a> {
        id: &'a str,
        cwd: &'a Path,
        recency_at_ms: Option<i64>,
        updated_at_ms: Option<i64>,
        updated_at: Option<i64>,
        archived: bool,
        pinned: bool,
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture demonstrates all interacting inventory filters and audit fields"
    )]
    fn inventory_applies_boundaries_normalization_filters_metadata_order_and_limit() {
        let connection = fixture_connection();
        let root = std::env::temp_dir().join("teamy-codex-inventory-fixture");
        let other_root = std::env::temp_dir().join("teamy-codex-inventory-other");
        let outside = PathBuf::from(format!("{}-outside", root.to_string_lossy()));
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "boundary",
                cwd: &root.join("boundary"),
                recency_at_ms: None,
                updated_at_ms: None,
                updated_at: Some(CUTOFF_MS / 1_000),
                archived: false,
                pinned: true,
            },
        );
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "before",
                cwd: &root.join("before"),
                recency_at_ms: Some(CUTOFF_MS - 1),
                updated_at_ms: None,
                updated_at: None,
                archived: false,
                pinned: false,
            },
        );
        for id in ["tie-a", "tie-b"] {
            insert_fixture(
                &connection,
                &FixtureTask {
                    id,
                    cwd: &root.join(id),
                    recency_at_ms: Some(CUTOFF_MS + 2_000),
                    updated_at_ms: None,
                    updated_at: None,
                    archived: false,
                    pinned: false,
                },
            );
        }
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "max-normalized",
                cwd: &other_root.join("max"),
                recency_at_ms: Some(CUTOFF_MS + 500),
                updated_at_ms: None,
                updated_at: Some(CUTOFF_MS / 1_000 + 4),
                archived: false,
                pinned: false,
            },
        );
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "prefix-is-not-under",
                cwd: &outside,
                recency_at_ms: Some(CUTOFF_MS + 9_000),
                updated_at_ms: None,
                updated_at: None,
                archived: false,
                pinned: false,
            },
        );
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "archived",
                cwd: &root.join("archived"),
                recency_at_ms: Some(CUTOFF_MS + 3_000),
                updated_at_ms: None,
                updated_at: None,
                archived: true,
                pinned: true,
            },
        );

        let filters = normalize_cwd_filters(&[display_path(&root), display_path(&other_root)])
            .expect("fixture filters should normalize");
        let columns = load_threads_columns(&connection).expect("fixture schema should load");
        let result = query_inventory(
            &connection,
            &columns,
            &InventoryQuery {
                cutoff_ms: CUTOFF_MS,
                cwd_under: &filters,
                include_archived: false,
                limit: None,
            },
        )
        .expect("fixture inventory should query");

        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| row.task_id.as_str())
                .collect::<Vec<_>>(),
            ["max-normalized", "tie-a", "tie-b", "boundary"]
        );
        let boundary = result
            .rows
            .iter()
            .find(|row| row.task_id == "boundary")
            .expect("cutoff boundary should be inclusive");
        assert_eq!(boundary.last_activity_ms, CUTOFF_MS);
        assert_eq!(boundary.last_activity_source, "updated_at");
        assert!(boundary.pinned);
        assert_eq!(boundary.agent_role.as_deref(), Some("worker"));
        assert_eq!(boundary.agent_path.as_deref(), Some("/root/worker"));
        assert_eq!(boundary.agent_name.as_deref(), Some("Fixture Agent"));

        let with_archived = query_inventory(
            &connection,
            &columns,
            &InventoryQuery {
                cutoff_ms: CUTOFF_MS,
                cwd_under: &filters,
                include_archived: true,
                limit: Some(2),
            },
        )
        .expect("archived fixture inventory should query");
        assert_eq!(with_archived.matched_count, 5);
        assert_eq!(with_archived.rows.len(), 2);
        assert_eq!(with_archived.truncated_count, 3);
        assert_eq!(with_archived.rows[0].task_id, "max-normalized");
        assert_eq!(with_archived.rows[1].task_id, "archived");
        assert!(with_archived.rows[1].archived);
        assert!(with_archived.rows[1].pinned);
    }

    #[test]
    fn unchanged_inventory_runs_have_the_same_semantic_row_digest() {
        let connection = fixture_connection();
        let root = std::env::temp_dir().join("teamy-codex-inventory-digest");
        insert_fixture(
            &connection,
            &FixtureTask {
                id: "stable",
                cwd: &root,
                recency_at_ms: Some(CUTOFF_MS),
                updated_at_ms: None,
                updated_at: None,
                archived: false,
                pinned: false,
            },
        );
        let filters =
            normalize_cwd_filters(&[display_path(&root)]).expect("fixture filter should normalize");
        let columns = load_threads_columns(&connection).expect("fixture schema should load");
        let query = InventoryQuery {
            cutoff_ms: CUTOFF_MS,
            cwd_under: &filters,
            include_archived: false,
            limit: None,
        };
        let first =
            query_inventory(&connection, &columns, &query).expect("first inventory should query");
        let second =
            query_inventory(&connection, &columns, &query).expect("second inventory should query");
        assert_eq!(
            semantic_rows_sha256(&first.rows).expect("first digest should hash"),
            semantic_rows_sha256(&second.rows).expect("second digest should hash")
        );
    }

    #[test]
    fn atomic_manifest_refuses_and_then_explicitly_replaces() {
        let root = std::env::temp_dir().join(format!(
            "teamy-codex-inventory-manifest-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("fixture clock should follow Unix epoch")
                .as_nanos()
        ));
        let path = root.join("manifest.json");
        write_manifest_atomic(&path, b"first", false).expect("first manifest should install");
        let refusal = write_manifest_atomic(&path, b"forbidden", false);
        assert_eq!(
            fs::read(&path).expect("refused manifest should remain readable"),
            b"first"
        );
        let error = refusal.expect_err("second manifest should be refused without overwrite");
        assert!(error.to_string().contains("already exists"));

        write_manifest_atomic(&path, b"replacement", true)
            .expect("explicit manifest replacement should install atomically");
        assert_eq!(
            fs::read(&path).expect("replacement manifest should be readable"),
            b"replacement"
        );
        fs::remove_dir_all(&root).expect("manifest fixture should be removable");
    }

    fn fixture_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("in-memory SQLite should open");
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    cwd TEXT NOT NULL,
                    recency_at_ms INTEGER,
                    recency_at INTEGER,
                    updated_at_ms INTEGER,
                    updated_at INTEGER,
                    created_at_ms INTEGER,
                    created_at INTEGER,
                    archived INTEGER NOT NULL,
                    is_pinned INTEGER NOT NULL,
                    agent_role TEXT,
                    agent_path TEXT,
                    agent_nickname TEXT,
                    model TEXT,
                    reasoning_effort TEXT,
                    tokens_used INTEGER,
                    source TEXT,
                    thread_source TEXT
                );",
            )
            .expect("fixture schema should be created");
        connection
    }

    fn insert_fixture(connection: &Connection, task: &FixtureTask<'_>) {
        connection
            .execute(
                "INSERT INTO threads (
                    id, title, cwd, recency_at_ms, recency_at, updated_at_ms,
                    updated_at, created_at_ms, created_at, archived, is_pinned,
                    agent_role, agent_path, agent_nickname, model,
                    reasoning_effort, tokens_used, source, thread_source
                ) VALUES (
                    ?1, ?2, ?3, ?4, 0, ?5, ?6, 0, 0, ?7, ?8,
                    'worker', '/root/worker', 'Fixture Agent', 'fixture-model',
                    'medium', 42, 'fixture', 'fixture-thread'
                )",
                params![
                    task.id,
                    format!("{} title", task.id),
                    display_path(task.cwd),
                    task.recency_at_ms,
                    task.updated_at_ms,
                    task.updated_at,
                    i64::from(task.archived),
                    i64::from(task.pinned),
                ],
            )
            .expect("fixture task should insert");
    }
}
