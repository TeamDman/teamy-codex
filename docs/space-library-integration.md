# ChatGPT Space and Library integration research

Status: documentation research only, 3 November 2026. Teamy Codex has no
Space or Library integration, background synchronization, or credentials for
this purpose. No private Space content was accessed during this investigation.

## Supported surfaces

[ChatGPT Space pages](https://learn.chatgpt.com/docs/space/pages) can be edited
by a user or with ChatGPT. The [plugin reference](https://developers.openai.com/plugins/reference)
documents a *ChatGPT-hosted widget* file picker (`window.openai.selectFiles`),
temporary download URLs for authorized selections, and an upload helper with an
optional Library save. Availability is optional and must be feature-detected.
It also documents explicit tool file inputs. These are user-authorized file
transfers inside a plugin flow, not a general native CLI API for enumerating or
editing every Space page. File IDs and temporary URLs should be treated as
opaque and sensitive.

The current Codex execution environment exposes first-party Library tools for
searching and preparing a user's authorized files for local use. Those
environment-provided tools do not establish a public endpoint or a stable
library that an independently launched `teamy-codex.exe` can invoke. The
reviewed [OpenAI API reference](https://developers.openai.com/api/reference/overview)
and Space guidance did not establish a public API for general Space page or
Library synchronization. This is a research finding, not proof that such an
API can never exist. Do not reverse engineer private web endpoints or reuse
browser session credentials.

[Codex app-server](https://developers.openai.com/codex/app-server) provides a
documented client protocol for Codex threads and command-execution items,
including output deltas. It is a separate integration from Space and from the
OTLP listener, and does not establish access to an unrelated existing ChatGPT
Work conversation. The [plugin/MCP guide](https://developers.openai.com/api/docs/mcp)
describes connecting ChatGPT or Codex to an app's tools and data; it does not
by itself grant an independent executable access to Space.

## Candidate user-directed bridge

If this is useful after the terminal receiver is qualified, consider a
ChatGPT-hosted plugin interface that lets the user explicitly select a file,
then passes an authorized file reference to a narrowly scoped tool. Treat
transport to the local CLI, authentication, availability, lifetime and user
consent as unresolved design decisions; do not expose the loopback telemetry
receiver to a remote plugin or assume a local bridge exists. Keep file import
and any requested export explicit, show destination and sensitivity, preserve
identity where supported, and avoid automatic collection or persistent access.
Validate with a deliberately nonsensitive synthetic file and an authorized
destination before considering real telemetry data.
