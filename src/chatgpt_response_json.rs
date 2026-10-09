//! Read-only extraction from a user-supplied `ChatGPT` conversation response.
//!
//! This is an intentionally narrow importer for an undocumented JSON shape, not
//! a client for `ChatGPT`'s browser-only backend. It never handles credentials.

use eyre::{Context, bail};
use facet::Facet;
use facet_json::RawJson;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const CONVERSATION_URL_PREFIX: &str = "https://chatgpt.com/c/";

#[derive(Facet, Debug)]
pub struct ChatgptTranscript {
    pub conversation_id: String,
    pub messages: Vec<ChatgptTranscriptMessage>,
}

#[derive(Facet, Debug)]
pub struct ChatgptTranscriptMessage {
    /// Zero-based position in the source response's `messages` array.
    pub source_index: usize,
    pub role: String,
    /// Preserve each text part exactly, without introducing separators.
    pub parts: Vec<String>,
}

#[derive(Facet)]
struct Response<'a> {
    conversation_id: RawJson<'a>,
    messages: Vec<RawJson<'a>>,
    page_info: RawJson<'a>,
    async_status: Option<RawJson<'a>>,
    context_truncation_continuation: Option<RawJson<'a>>,
}

#[derive(Facet)]
struct PageInfo {
    has_previous_page: bool,
    has_next_page: bool,
}

#[derive(Facet)]
struct Message<'a> {
    id: RawJson<'a>,
    author: RawJson<'a>,
    channel: Option<RawJson<'a>>,
    recipient: Option<RawJson<'a>>,
    end_turn: Option<RawJson<'a>>,
    status: Option<RawJson<'a>>,
    content: Option<RawJson<'a>>,
}

#[derive(Facet)]
struct Author {
    role: String,
}

#[derive(Facet)]
struct TextContent<'a> {
    content_type: String,
    parts: Vec<RawJson<'a>>,
}

/// Read at most one complete local response file and extract user/final text.
///
/// # Errors
///
/// Returns an error for an oversized or malformed file, incomplete pagination,
/// unsupported selected content, or a mismatched conversation URL.
pub fn read_chatgpt_response(
    path: &Path,
    conversation_url: Option<&str>,
) -> eyre::Result<ChatgptTranscript> {
    let file = File::open(path).wrap_err_with(|| format!("could not open {}", path.display()))?;
    let contents = read_bounded(file)
        .wrap_err_with(|| format!("could not read UTF-8 JSON from {}", path.display()))?;
    parse_chatgpt_response(&contents, conversation_url)
}

fn read_bounded(reader: impl Read) -> eyre::Result<String> {
    let mut contents = String::new();
    reader
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_string(&mut contents)
        .wrap_err("invalid UTF-8 in ChatGPT response")?;
    if contents.len() as u64 > MAX_RESPONSE_BYTES {
        bail!("ChatGPT response exceeds the {MAX_RESPONSE_BYTES}-byte input limit");
    }
    Ok(contents)
}

fn parse_chatgpt_response(
    contents: &str,
    conversation_url: Option<&str>,
) -> eyre::Result<ChatgptTranscript> {
    let response: Response<'_> =
        facet_json::from_str_borrowed(contents).wrap_err("invalid ChatGPT response JSON")?;
    let conversation_id = json_string(&response.conversation_id, "conversation_id")?;
    if !is_uuid(&conversation_id) {
        bail!("response conversation_id is not a UUID");
    }
    if let Some(url) = conversation_url {
        validate_conversation_url(url, &conversation_id)?;
    }

    let page_info: PageInfo = facet_json::from_str(response.page_info.as_str())
        .wrap_err("response is missing valid page_info pagination flags")?;
    if page_info.has_previous_page || page_info.has_next_page {
        bail!("response contains only one page; supply a complete conversation response");
    }
    if has_non_null_value(response.async_status.as_ref())
        || has_non_null_value(response.context_truncation_continuation.as_ref())
    {
        bail!("response reports an unfinished asynchronous or truncated continuation");
    }

    let mut messages = Vec::new();
    let mut seen_ids = HashSet::new();
    for (source_index, raw) in response.messages.iter().enumerate() {
        let message: Message<'_> = facet_json::from_str_borrowed(raw.as_str())
            .wrap_err_with(|| format!("invalid message at source index {source_index}"))?;
        let author: Author = facet_json::from_str(message.author.as_str())
            .wrap_err_with(|| format!("invalid author at source index {source_index}"))?;
        let channel = optional_json_string(message.channel.as_ref(), "channel")?;
        let selected_role = match author.role.as_str() {
            "user" => Some("user"),
            "assistant" if channel.as_deref() == Some("final") => Some("assistant"),
            _ => None,
        };
        let Some(role) = selected_role else {
            continue;
        };

        let id = json_string(&message.id, "message id")?;
        if !seen_ids.insert(id) {
            bail!("response contains a duplicate selected message id");
        }
        if role == "assistant" {
            let recipient = optional_json_string(message.recipient.as_ref(), "recipient")?;
            let status = optional_json_string(message.status.as_ref(), "status")?;
            let ended = message
                .end_turn
                .as_ref()
                .map(|value| facet_json::from_str::<bool>(value.as_str()))
                .transpose()
                .wrap_err("invalid assistant end_turn value")?;
            if recipient.as_deref() != Some("all")
                || status.as_deref() != Some("finished_successfully")
                || ended != Some(true)
            {
                bail!("assistant final at source index {source_index} is incomplete");
            }
        }

        let raw_content = message.content.as_ref().ok_or_else(|| {
            eyre::eyre!("selected message at source index {source_index} has no content")
        })?;
        let content: TextContent<'_> = facet_json::from_str_borrowed(raw_content.as_str())
            .wrap_err_with(|| format!("invalid selected content at source index {source_index}"))?;
        if content.content_type != "text" {
            bail!(
                "selected message at source index {source_index} has non-text content type {}",
                content.content_type
            );
        }
        let mut parts = Vec::with_capacity(content.parts.len());
        for (part_index, part) in content.parts.iter().enumerate() {
            let text: String = facet_json::from_str(part.as_str()).wrap_err_with(|| {
                format!(
                    "selected message at source index {source_index} has non-text part {part_index}"
                )
            })?;
            parts.push(text);
        }
        messages.push(ChatgptTranscriptMessage {
            source_index,
            role: role.to_owned(),
            parts,
        });
    }
    Ok(ChatgptTranscript {
        conversation_id,
        messages,
    })
}

fn json_string(raw: &RawJson<'_>, field: &str) -> eyre::Result<String> {
    facet_json::from_str(raw.as_str()).wrap_err_with(|| format!("invalid {field} string"))
}

fn optional_json_string(raw: Option<&RawJson<'_>>, field: &str) -> eyre::Result<Option<String>> {
    raw.filter(|value| value.as_str().trim() != "null")
        .map(|value| json_string(value, field))
        .transpose()
}

fn has_non_null_value(raw: Option<&RawJson<'_>>) -> bool {
    raw.is_some_and(|value| value.as_str().trim() != "null")
}

fn validate_conversation_url(url: &str, conversation_id: &str) -> eyre::Result<()> {
    let candidate = url
        .strip_prefix(CONVERSATION_URL_PREFIX)
        .map(|path| path.strip_suffix('/').unwrap_or(path))
        .ok_or_else(|| eyre::eyre!("conversation URL must use https://chatgpt.com/c/<uuid>"))?;
    if !is_uuid(candidate) {
        bail!("conversation URL must use https://chatgpt.com/c/<uuid>");
    }
    if !candidate.eq_ignore_ascii_case(conversation_id) {
        bail!("conversation URL does not match response conversation_id");
    }
    Ok(())
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "11111111-2222-4333-8444-555555555555";
    const URL: &str = "https://chatgpt.com/c/11111111-2222-4333-8444-555555555555";

    fn response(messages: &str, page_info: &str) -> String {
        format!(r#"{{"conversation_id":"{ID}","messages":[{messages}],"page_info":{page_info}}}"#)
    }

    fn complete_page() -> &'static str {
        r#"{"has_previous_page":false,"has_next_page":false}"#
    }

    #[test]
    fn extracts_only_user_and_completed_final_text_in_source_order() {
        let raw = response(
            r#"{"id":"system","author":{"role":"system"},"content":{"content_type":"text","parts":["hidden"]}},
               {"id":"user-1","author":{"role":"user"},"content":{"content_type":"text","parts":["first", "second"]}},
               {"id":"thought","author":{"role":"assistant"},"content":{"content_type":"thoughts","parts":[{"secret":true}]}},
               {"id":"tool","author":{"role":"tool"},"content":{"content_type":"text","parts":["hidden"]}},
               {"id":"final-1","author":{"role":"assistant"},"channel":"final","recipient":"all","end_turn":true,"status":"finished_successfully","content":{"content_type":"text","parts":["answer"]}}"#,
            complete_page(),
        );
        let transcript = parse_chatgpt_response(&raw, Some(URL)).expect("extract transcript");
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0].source_index, 1);
        assert_eq!(transcript.messages[0].role, "user");
        assert_eq!(transcript.messages[0].parts, ["first", "second"]);
        assert_eq!(transcript.messages[1].source_index, 4);
        assert_eq!(transcript.messages[1].role, "assistant");
        assert_eq!(transcript.messages[1].parts, ["answer"]);
    }

    #[test]
    fn refuses_partial_pagination_and_continuations() {
        let partial_page = response("", r#"{"has_previous_page":true,"has_next_page":false}"#);
        let partial_error =
            parse_chatgpt_response(&partial_page, None).expect_err("partial page must fail");
        assert!(partial_error.to_string().contains("only one page"));
        let later_page = response("", r#"{"has_previous_page":false,"has_next_page":true}"#);
        let later_error =
            parse_chatgpt_response(&later_page, None).expect_err("later page must fail");
        assert!(later_error.to_string().contains("only one page"));
        let continuation = response("", complete_page()).replace(
            "\"page_info\"",
            "\"context_truncation_continuation\":{\"cursor\":\"next\"},\"page_info\"",
        );
        let continuation_error =
            parse_chatgpt_response(&continuation, None).expect_err("continuation must fail");
        assert!(continuation_error.to_string().contains("continuation"));
    }

    #[test]
    fn preserves_consecutive_users_and_final_replies() {
        let raw = response(
            r#"{"id":"u1","author":{"role":"user"},"content":{"content_type":"text","parts":["first"]}},
               {"id":"u2","author":{"role":"user"},"content":{"content_type":"text","parts":["second"]}},
               {"id":"a1","author":{"role":"assistant"},"channel":"final","recipient":"all","end_turn":true,"status":"finished_successfully","content":{"content_type":"text","parts":["first reply"]}},
               {"id":"a2","author":{"role":"assistant"},"channel":"final","recipient":"all","end_turn":true,"status":"finished_successfully","content":{"content_type":"text","parts":["second reply"]}}"#,
            complete_page(),
        );
        let transcript = parse_chatgpt_response(&raw, None).expect("preserve source order");
        assert_eq!(transcript.messages.len(), 4);
        assert_eq!(transcript.messages[0].parts, ["first"]);
        assert_eq!(transcript.messages[1].parts, ["second"]);
        assert_eq!(transcript.messages[2].parts, ["first reply"]);
        assert_eq!(transcript.messages[3].parts, ["second reply"]);
    }

    #[test]
    fn refuses_incomplete_final_message() {
        let incomplete = response(
            r#"{"id":"u","author":{"role":"user"},"content":{"content_type":"text","parts":["question"]}},
               {"id":"a","author":{"role":"assistant"},"channel":"final","recipient":"all","end_turn":false,"status":"in_progress","content":{"content_type":"text","parts":["partial"]}}"#,
            complete_page(),
        );
        let error =
            parse_chatgpt_response(&incomplete, None).expect_err("incomplete final must fail");
        assert!(error.to_string().contains("incomplete"));
    }

    #[test]
    fn refuses_selected_non_text_parts() {
        let raw = response(
            r#"{"id":"u","author":{"role":"user"},"content":{"content_type":"text","parts":["text",{"image":"redacted"}]}}"#,
            complete_page(),
        );
        let error = parse_chatgpt_response(&raw, None).expect_err("non-text part must fail");
        assert!(error.to_string().contains("non-text part"));
    }

    #[test]
    fn refuses_mismatched_or_noncanonical_url() {
        let raw = response("", complete_page());
        let mismatch = parse_chatgpt_response(
            &raw,
            Some("https://chatgpt.com/c/aaaaaaaa-2222-4333-8444-555555555555"),
        )
        .expect_err("mismatched URL must fail");
        assert!(mismatch.to_string().contains("does not match"));
        let noncanonical = parse_chatgpt_response(
            &raw,
            Some("https://evil.example/c/11111111-2222-4333-8444-555555555555"),
        )
        .expect_err("noncanonical URL must fail");
        assert!(noncanonical.to_string().contains("conversation URL must"));
    }

    #[test]
    fn refuses_duplicate_selected_message_ids() {
        let raw = response(
            r#"{"id":"same","author":{"role":"user"},"content":{"content_type":"text","parts":["first"]}},
               {"id":"same","author":{"role":"user"},"content":{"content_type":"text","parts":["second"]}}"#,
            complete_page(),
        );
        let error = parse_chatgpt_response(&raw, None).expect_err("duplicate IDs must fail");
        assert!(error.to_string().contains("duplicate selected message id"));
    }

    #[test]
    fn refuses_oversized_input_before_parsing() {
        let input = std::io::repeat(b' ').take(MAX_RESPONSE_BYTES + 1);
        let error = read_bounded(input).expect_err("oversized input must fail");
        assert!(error.to_string().contains("input limit"));
    }
}
