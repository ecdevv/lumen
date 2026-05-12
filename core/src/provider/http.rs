//! OpenAI-compatible HTTP provider.
//!
//! Speaks the OpenAI chat-completions wire format over HTTP/HTTPS.
//! Works against:
//!   * llama.cpp's `llama-server` (`http://localhost:8080`),
//!   * Ollama's OpenAI compatibility shim,
//!   * vLLM, LM Studio, OpenAI itself, etc.
//!
//! Streaming-only: requests always set `stream: true` and parse the
//! Server-Sent-Events response into [`Chunk`]s.

use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::stream::{self, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use super::{
    Chunk, ChunkStream, CompletionRequest, FinishReason, Message, Provider, Role, ToolCallDelta,
    ToolSchema,
};

/// HTTP provider against an OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct HttpProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl HttpProvider {
    /// Build a provider against `base_url` (e.g. `http://localhost:8080`).
    /// Pass `api_key = None` for local llama.cpp; bearer auth is sent
    /// when a key is present.
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Result<Self> {
        // No *request* timeout: completions can legitimately take many
        // minutes on a slow local model; cancellation comes from the
        // caller dropping the stream. But we do cap the *connect*
        // phase - a down server should surface in seconds, not freeze
        // the UI invisibly.
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
            api_key,
        })
    }
}

// --- Wire types ----------------------------------------------------- //
// Private to this module so the public API in `provider/mod.rs` doesn't
// leak any one backend's schema. Mapping happens here only.
//
// OpenAI's chat-completions wire format wraps each tool - and each tool
// call inside an assistant message - in a `{"type": "function",
// "function": {...}}` envelope. The abstract `ToolSchema` / `ToolCall`
// in `provider/mod.rs` deliberately omits that envelope so future
// non-OpenAI providers (Anthropic, Gemini) can transform their own way.
// We rebuild the OpenAI shape on the way out and the way back in.

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct WireTool<'a> {
    /// Always `"function"` for the chat-completions tools API. Other
    /// values exist (e.g. `"web_search"` on hosted OpenAI) but lumen
    /// only emits user-defined function tools.
    #[serde(rename = "type")]
    kind: &'static str,
    function: &'a ToolSchema,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: Role,
    /// Explicit `Option<&str>` (no `skip_serializing_if`) so this
    /// serializes as `null` when the assistant produced only tool
    /// calls - strict OpenAI-compat servers (e.g. llama.cpp) reject
    /// `content: ""` for tool-calling messages.
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<WireToolCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct WireToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunctionCall<'a>,
}

#[derive(Serialize)]
struct WireFunctionCall<'a> {
    name: &'a str,
    /// JSON-encoded argument string, exactly as the model produced it.
    arguments: &'a str,
}

#[derive(Deserialize)]
struct WireResponseChunk {
    #[serde(default)]
    choices: Vec<WireChoice>,
}

#[derive(Deserialize)]
struct WireChoice {
    #[serde(default)]
    delta: WireDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Deserialize)]
struct WireToolCallDelta {
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireFunctionDelta>,
}

#[derive(Deserialize, Default)]
struct WireFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// -------------------------------------------------------------------- //

#[async_trait]
impl Provider for HttpProvider {
    async fn complete<'a>(&self, req: CompletionRequest<'a>) -> Result<ChunkStream> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let body = build_wire_request(&req);

        let mut builder = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let resp = builder.send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::ProviderStatus { status, body });
        }

        // Convert byte stream -> SSE event stream -> flat-mapped chunks.
        // `eventsource-stream` does the line-buffering and SSE field
        // parsing; we just interpret the `data:` payload of each event.
        let event_stream = resp.bytes_stream().eventsource();

        let chunk_stream = event_stream.flat_map(|item| {
            let chunks = match item {
                Ok(event) => parse_event(&event.data),
                Err(e) => vec![Err(Error::Sse(e.to_string()))],
            };
            stream::iter(chunks)
        });

        Ok(Box::pin(chunk_stream))
    }

    /// Fetch the model list from the OpenAI-compatible `/v1/models`
    /// endpoint. Response shape (per OpenAI / llama-server):
    /// `{ "object": "list", "data": [{ "id": "...", ... }, ...] }`.
    /// We only need the `id` field of each entry.
    async fn list_models(&self) -> Result<Vec<String>> {
        // Per-request timeout (no overall-timeout on the client - see
        // `new`). `/v1/models` is a one-shot JSON read; a wedged
        // server that accepts the connection but never responds
        // would otherwise park the model picker indefinitely.
        const LIST_MODELS_TIMEOUT: Duration = Duration::from_secs(30);

        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let mut builder = self.client.get(&url).timeout(LIST_MODELS_TIMEOUT);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        let resp = builder.send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::ProviderStatus { status, body });
        }
        let payload: ModelsResponse = resp.json().await?;
        Ok(payload.data.into_iter().map(|m| m.id).collect())
    }
}

/// Wire shape of `GET /v1/models`. Local `serde` types - we don't
/// surface this type outside the provider since callers only need
/// the flat `Vec<String>` of ids.
#[derive(serde::Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(serde::Deserialize)]
struct ModelEntry {
    id: String,
}

/// Translate the abstract [`CompletionRequest`] into the OpenAI
/// chat-completions wire shape (function-envelope wrappers, `null`
/// content where appropriate, etc.). Borrows from `req` throughout -
/// no clones of the message body or tool schemas.
fn build_wire_request<'a>(req: &'a CompletionRequest<'a>) -> WireRequest<'a> {
    let messages = req.messages.iter().map(wire_message).collect();
    let tools = req
        .tools
        .iter()
        .map(|t| WireTool {
            kind: "function",
            function: t,
        })
        .collect();
    WireRequest {
        model: &req.model,
        messages,
        tools,
        stream: true,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
    }
}

fn wire_message(m: &Message) -> WireMessage<'_> {
    // Assistant turns that produced only tool calls have `content == ""`
    // in our internal model; the wire form must send `null`. Otherwise
    // we forward content verbatim.
    let content = if m.content.is_empty() && m.tool_calls.is_some() {
        None
    } else {
        Some(m.content.as_str())
    };
    let tool_calls = m.tool_calls.as_ref().map(|calls| {
        calls
            .iter()
            .map(|c| WireToolCall {
                id: &c.id,
                kind: "function",
                function: WireFunctionCall {
                    name: &c.name,
                    arguments: &c.arguments,
                },
            })
            .collect()
    });
    WireMessage {
        role: m.role,
        content,
        tool_calls,
        tool_call_id: m.tool_call_id.as_deref(),
    }
}

/// Convert one SSE event payload into 0+ chunks. Returns `Vec` because
/// a single event can carry multiple parallel tool-call deltas.
fn parse_event(data: &str) -> Vec<Result<Chunk>> {
    // OpenAI marks end-of-stream with the literal string `[DONE]` -
    // not JSON, so we special-case it before parsing.
    if data.trim() == "[DONE]" {
        return vec![Ok(Chunk::Done(FinishReason::Stop))];
    }

    let parsed: WireResponseChunk = match serde_json::from_str(data) {
        Ok(p) => p,
        Err(e) => return vec![Err(Error::Json(e))],
    };

    let Some(choice) = parsed.choices.into_iter().next() else {
        return vec![];
    };

    if let Some(reason) = choice.finish_reason {
        return vec![Ok(Chunk::Done(parse_finish_reason(&reason)))];
    }

    let mut chunks = Vec::new();

    if let Some(content) = choice.delta.content.filter(|c| !c.is_empty()) {
        chunks.push(Ok(Chunk::Content(content)));
    }

    if let Some(calls) = choice.delta.tool_calls {
        for call in calls {
            let func = call.function.unwrap_or_default();
            chunks.push(Ok(Chunk::ToolCallDelta(ToolCallDelta {
                index: call.index,
                id: call.id,
                name: func.name,
                arguments: func.arguments,
            })));
        }
    }

    chunks
}

fn parse_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        _ => FinishReason::Other,
    }
}

#[cfg(test)]
#[path = "../tests/provider/http.rs"]
mod tests;
