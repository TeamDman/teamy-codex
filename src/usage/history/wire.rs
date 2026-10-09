use super::super::records::UsageTokens;
use facet::Facet;
use facet_json::RawJson;

#[derive(Facet)]
pub(super) struct Envelope<'a> {
    #[facet(rename = "type")]
    pub kind: String,
    pub timestamp: Option<String>,
    pub ordinal: Option<u64>,
    pub payload: RawJson<'a>,
}

#[derive(Facet, Clone, Debug)]
pub(super) struct HistoryBase {
    pub thread_id: String,
    pub end_byte_offset: u64,
    pub end_ordinal_exclusive: u64,
}

#[derive(Facet, Debug)]
pub(super) struct Header {
    pub id: String,
    pub cli_version: Option<String>,
    pub history_base: Option<HistoryBase>,
    pub forked_from_id: Option<String>,
    pub parent_thread_id: Option<String>,
    pub forked_from_ordinal_exclusive: Option<u64>,
    pub subagent_history_start_ordinal: Option<u64>,
}

#[derive(Facet, Clone, Default)]
pub(super) struct Context {
    pub turn_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Facet)]
pub(super) struct ResponseUsage {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub response_id: String,
    pub usage: Option<UsageTokens>,
    pub effective_model: Option<String>,
    pub effective_effort: Option<String>,
}

#[derive(Facet)]
pub(super) struct Event {
    #[facet(rename = "type")]
    pub kind: String,
    pub info: Option<CounterInfo>,
}

#[derive(Facet)]
pub(super) struct CounterInfo {
    pub total_token_usage: UsageTokens,
    pub last_token_usage: UsageTokens,
}
