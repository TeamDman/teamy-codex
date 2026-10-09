use super::types::HistorySource;
use super::wire::{Envelope, Header, HistoryBase};
use eyre::{Context, bail};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub(super) const MAX_LINE: u64 = 32 * 1024 * 1024;

pub(super) struct Root {
    pub id: String,
    pub path: PathBuf,
    pub counter: i64,
}
pub(super) struct Storage {
    pub roots: Vec<Root>,
    pub files: BTreeMap<String, Vec<PathBuf>>,
    pub boundaries: Vec<PathBuf>,
}

impl Storage {
    pub fn open(home: &Path, threads: &[String]) -> eyre::Result<Self> {
        let db = home.join("state_5.sqlite");
        let connection = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .wrap_err("cannot open native history metadata read-only")?;
        connection.execute_batch("PRAGMA query_only = ON;")?;
        let mut stmt =
            connection.prepare("SELECT id, rollout_path, tokens_used FROM threads ORDER BY id")?;
        let all = stmt
            .query_map([], |r| {
                Ok(Root {
                    id: r.get(0)?,
                    path: PathBuf::from(r.get::<_, String>(1)?),
                    counter: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let roots = all
            .into_iter()
            .filter(|r| threads.is_empty() || threads.contains(&r.id))
            .collect();
        let mut storage = Self {
            roots,
            files: BTreeMap::new(),
            boundaries: Vec::new(),
        };
        for name in ["sessions", "archived_sessions"] {
            let directory = home.join(name);
            if directory.exists() {
                storage.boundaries.push(directory.canonicalize()?);
                storage.index(&directory)?;
            }
        }
        Ok(storage)
    }

    fn index(&mut self, directory: &Path) -> eyre::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                self.index(&entry.path())?;
            } else if kind.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let stem = name
                    .strip_suffix(".jsonl")
                    .or_else(|| name.strip_suffix(".jsonl.zst"));
                if let Some(stem) = stem.filter(|s| s.starts_with("rollout-"))
                    && let Some(id) = stem.get(stem.len().saturating_sub(36)..)
                {
                    self.files
                        .entry(id.to_owned())
                        .or_default()
                        .push(entry.path());
                }
            }
        }
        Ok(())
    }

    pub fn allowed(&self, path: &Path) -> eyre::Result<PathBuf> {
        let path = path.canonicalize()?;
        if !self
            .boundaries
            .iter()
            .any(|boundary| path.starts_with(boundary))
        {
            bail!("rollout path is outside the selected Codex sessions/archived_sessions roots");
        }
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            bail!("compressed rollout history is not supported by this reader");
        }
        Ok(path)
    }

    pub fn base_path(&self, base: &HistoryBase) -> eyre::Result<PathBuf> {
        match self.files.get(&base.thread_id).map(Vec::as_slice) {
            Some([path]) => self.allowed(path),
            Some(_) => bail!("ambiguous history-base rollout ID {}", base.thread_id),
            None => bail!("missing history-base rollout ID {}", base.thread_id),
        }
    }
}

pub(super) fn read_header(path: &Path) -> eyre::Result<Header> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    reader
        .by_ref()
        .take(MAX_LINE + 1)
        .read_until(b'\n', &mut line)?;
    if u64::try_from(line.len())? > MAX_LINE {
        bail!("oversized session metadata header");
    }
    let text = std::str::from_utf8(&line)
        .map_err(|_private_parse_error| eyre::eyre!("invalid UTF-8 session metadata header"))?;
    let envelope: Envelope<'_> = facet_json::from_str_borrowed(text)
        .map_err(|_private_parse_error| eyre::eyre!("malformed session metadata header"))?;
    if envelope.kind != "session_meta" {
        bail!("missing session_meta header");
    }
    facet_json::from_str(envelope.payload.as_str())
        .map_err(|_private_parse_error| eyre::eyre!("unsupported session metadata header"))
}

pub(super) fn next_line(
    reader: &mut impl BufRead,
    line: &mut Vec<u8>,
    source: &mut HistorySource,
) -> eyre::Result<bool> {
    line.clear();
    let count = reader.by_ref().take(MAX_LINE + 1).read_until(b'\n', line)?;
    if count == 0 {
        return Ok(false);
    }
    source.lines_scanned += 1;
    source.bytes_scanned += u64::try_from(count)?;
    if u64::try_from(count)? > MAX_LINE {
        source.oversized_lines += 1;
        if !line.ends_with(b"\n") {
            source.bytes_scanned += u64::try_from(reader.skip_until(b'\n')?)?;
        }
        line.clear();
    }
    Ok(true)
}
