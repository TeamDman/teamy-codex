use super::super::{
    parse_time,
    records::{UsageRecord, UsageTokens},
};
use super::storage::next_line;
use super::types::HistorySource;
use super::wire::{Context, Envelope, Event, Header, ResponseUsage};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use teamy_cancellation::CancellationToken;

pub(super) struct Candidate {
    pub record: UsageRecord,
    pub turn: Option<String>,
    pub modern: bool,
}
#[derive(Default)]
pub(super) struct ScanState {
    pub contexts: BTreeMap<String, Context>,
    pub current: Context,
    pub counters: BTreeMap<String, UsageTokens>,
    pub last_counters: BTreeMap<String, i64>,
    pub reset_epochs: BTreeMap<String, usize>,
    pub records: Vec<Candidate>,
}

impl ScanState {
    pub fn scan(
        &mut self,
        path: &Path,
        header: &Header,
        limit: u64,
        ordinal_limit: Option<u64>,
        owner: &str,
        source: &mut HistorySource,
        cancellation: &CancellationToken,
    ) -> eyre::Result<()> {
        let mut reader = BufReader::new(File::open(path)?.take(limit));
        let mut bytes = Vec::new();
        while next_line(&mut reader, &mut bytes, source)? {
            cancellation.bail_if_cancelled()?;
            if bytes.is_empty() {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                source.malformed_metadata_lines += 1;
                continue;
            };
            // Narrow schemas only. Conversation payloads are never retained or emitted.
            if ![
                "turn_context",
                "token_usage_record",
                "token_count",
                "compacted",
            ]
            .iter()
            .any(|kind| text.contains(kind))
            {
                continue;
            }
            let parsed: Result<Envelope<'_>, _> = facet_json::from_str_borrowed(text);
            let Ok(envelope) = parsed else {
                if bytes.ends_with(b"\n") {
                    source.malformed_metadata_lines += 1;
                } else {
                    source.incomplete_trailing_lines += 1;
                }
                continue;
            };
            if ordinal_limit.is_some_and(|end| envelope.ordinal.is_some_and(|n| n >= end)) {
                break;
            }
            let result = self.event(&envelope, header, owner, source);
            if result.is_err() {
                source.malformed_metadata_lines += 1;
            }
        }
        Ok(())
    }

    fn event(
        &mut self,
        envelope: &Envelope<'_>,
        header: &Header,
        owner: &str,
        source: &mut HistorySource,
    ) -> eyre::Result<()> {
        match envelope.kind.as_str() {
            "turn_context" => {
                let context: Context = facet_json::from_str(envelope.payload.as_str())?;
                if header.id == owner {
                    if let Some(turn) = &context.turn_id {
                        self.contexts.insert(turn.clone(), context.clone());
                    }
                    self.current = context;
                }
            }
            "token_usage_record" => {
                let response: ResponseUsage = facet_json::from_str(envelope.payload.as_str())?;
                source.response_records += 1;
                if response.thread_id != owner {
                    source.inherited_records_excluded += 1;
                    return Ok(());
                }
                let usage = response
                    .usage
                    .ok_or_else(|| eyre::eyre!("missing response usage"))?;
                usage.validate()?;
                if response.response_id.trim().is_empty() {
                    eyre::bail!("missing response identity");
                }
                let context = response
                    .turn_id
                    .as_ref()
                    .and_then(|turn| self.contexts.get(turn));
                if context.is_none() && response.effective_model.is_none() {
                    source.unknown_context_records += 1;
                }
                let record = UsageRecord {
                    response_id: response.response_id,
                    thread_id: response.thread_id,
                    completed_at: valid_time(envelope.timestamp.as_deref()),
                    effective_model: response
                        .effective_model
                        .or_else(|| context.and_then(|c| c.model.clone())),
                    effective_effort: response
                        .effective_effort
                        .or_else(|| context.and_then(|c| c.effort.clone())),
                    tokens: usage,
                };
                self.records.push(Candidate {
                    record,
                    turn: response.turn_id,
                    modern: true,
                });
            }
            "event_msg" => {
                let event: Event = facet_json::from_str(envelope.payload.as_str())?;
                if event.kind == "token_count" {
                    source.token_count_events += 1;
                    if header.id != owner || inherited_legacy(envelope, header) {
                        source.inherited_records_excluded += 1;
                    } else if let Some(info) = event.info {
                        self.counter(
                            &info.total_token_usage,
                            &info.last_token_usage,
                            envelope,
                            owner,
                            source,
                        );
                    }
                }
            }
            "compacted" => source.compactions += 1,
            _ => {}
        }
        Ok(())
    }

    fn counter(
        &mut self,
        total: &UsageTokens,
        last: &UsageTokens,
        envelope: &Envelope<'_>,
        owner: &str,
        source: &mut HistorySource,
    ) {
        self.last_counters
            .insert(owner.to_owned(), total.total_tokens);
        let previous = self.counters.insert(owner.to_owned(), total.clone());
        if previous.as_ref() == Some(total) {
            source.repeated_counter_events += 1;
            return;
        }
        let delta = if let Some(previous) = previous {
            let delta = subtract(total, &previous);
            if delta.is_none() {
                source.counter_resets += 1;
                *self.reset_epochs.entry(owner.to_owned()).or_default() += 1;
                return;
            }
            delta
        } else if total == last {
            Some(total.clone())
        } else {
            None
        };
        let Some(delta) = delta.filter(|d| d == last && d.validate().is_ok()) else {
            source.ambiguous_counter_changes += 1;
            return;
        };
        if delta.total_tokens == 0 {
            return;
        }
        let epoch = self.reset_epochs.get(owner).copied().unwrap_or_default();
        let record = UsageRecord {
            response_id: format!(
                "legacy:{owner}:{epoch}:{}:{}:{}:{}:{}",
                total.input_tokens,
                total.cached_input_tokens,
                total.output_tokens,
                total.reasoning_output_tokens,
                total.total_tokens
            ),
            thread_id: owner.to_owned(),
            completed_at: valid_time(envelope.timestamp.as_deref()),
            effective_model: self.current.model.clone(),
            effective_effort: self.current.effort.clone(),
            tokens: delta,
        };
        self.records.push(Candidate {
            record,
            turn: self.current.turn_id.clone(),
            modern: false,
        });
    }
}

fn valid_time(value: Option<&str>) -> String {
    value
        .and_then(|s| parse_time(s).ok())
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn inherited_legacy(envelope: &Envelope<'_>, header: &Header) -> bool {
    let boundary = header
        .subagent_history_start_ordinal
        .or(header.forked_from_ordinal_exclusive);
    if let Some(boundary) = boundary {
        return envelope.ordinal.is_none_or(|ordinal| ordinal < boundary);
    }
    header.forked_from_id.is_some() || header.parent_thread_id.is_some()
}

fn subtract(current: &UsageTokens, previous: &UsageTokens) -> Option<UsageTokens> {
    let values = [
        current.input_tokens.checked_sub(previous.input_tokens)?,
        current
            .cached_input_tokens
            .checked_sub(previous.cached_input_tokens)?,
        current.output_tokens.checked_sub(previous.output_tokens)?,
        current
            .reasoning_output_tokens
            .checked_sub(previous.reasoning_output_tokens)?,
        current.total_tokens.checked_sub(previous.total_tokens)?,
    ];
    if values.iter().any(|n| *n < 0) {
        return None;
    }
    Some(UsageTokens {
        input_tokens: values[0],
        cached_input_tokens: values[1],
        output_tokens: values[2],
        reasoning_output_tokens: values[3],
        total_tokens: values[4],
    })
}
