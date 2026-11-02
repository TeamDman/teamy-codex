use super::CodexEvent;
use std::collections::BTreeMap;

/// Confidence is explicit when the producer instance was not identified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Correlation {
    Exact,
    SourceUnknown,
    MissingIdentity,
    Ambiguous,
    Unmatched,
    OtherEvent,
}

/// Indices refer to the original records, which remain available unmodified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallRow {
    pub decision: Option<usize>,
    pub result: Option<usize>,
    pub other: Option<usize>,
    pub correlation: Correlation,
}

/// Pair one decision with one result only when their session and call IDs match.
/// Duplicate decisions or results remain separate; no event is silently discarded.
#[must_use]
pub fn correlate(events: &[CodexEvent]) -> Vec<ToolCallRow> {
    type Key = (Option<String>, String, String);
    let mut groups: BTreeMap<Key, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    let mut rows = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let decision = event.name == "codex.tool_decision";
        let result = event.name == "codex.tool_result";
        if !decision && !result {
            rows.push(ToolCallRow {
                decision: None,
                result: None,
                other: Some(index),
                correlation: Correlation::OtherEvent,
            });
            continue;
        }
        let (Some(session), Some(call_id)) = (
            event.session.as_ref().filter(|s| !s.is_empty()),
            event.call_id.as_ref().filter(|s| !s.is_empty()),
        ) else {
            rows.push(single(index, decision, Correlation::MissingIdentity));
            continue;
        };
        let source = event
            .source
            .as_ref()
            .filter(|source| !source.is_empty())
            .cloned();
        let group = groups
            .entry((source, session.clone(), call_id.clone()))
            .or_default();
        if decision {
            group.0.push(index);
        } else {
            group.1.push(index);
        }
    }
    for ((source, _, _), (decisions, results)) in groups {
        if decisions.len() == 1 && results.len() == 1 {
            rows.push(ToolCallRow {
                decision: Some(decisions[0]),
                result: Some(results[0]),
                other: None,
                correlation: if source.is_some() {
                    Correlation::Exact
                } else {
                    Correlation::SourceUnknown
                },
            });
        } else {
            let correlation = if decisions.len() > 1 || results.len() > 1 {
                Correlation::Ambiguous
            } else {
                Correlation::Unmatched
            };
            rows.extend(
                decisions
                    .into_iter()
                    .map(|index| single(index, true, correlation)),
            );
            rows.extend(
                results
                    .into_iter()
                    .map(|index| single(index, false, correlation)),
            );
        }
    }
    rows.sort_by_key(|row| {
        row.decision
            .into_iter()
            .chain(row.result)
            .chain(row.other)
            .min()
            .unwrap_or(usize::MAX)
    });
    rows
}

fn single(index: usize, decision: bool, correlation: Correlation) -> ToolCallRow {
    ToolCallRow {
        decision: decision.then_some(index),
        result: (!decision).then_some(index),
        other: None,
        correlation,
    }
}
