//! LLM provider abstraction.
//!
//! The [`Provider`] trait is the boundary between `lumen-core` and any
//! specific LLM backend (local llama.cpp, OpenAI, Anthropic, Gemini, ...).
//! Currently one impl ships: an OpenAI-compatible HTTP client in the
//! sibling `http` module. The agent loop is written against the trait,
//! so adding backends later is just another impl.
//!
//! # Streaming
//! Completions stream as a sequence of [`Chunk`]s. The agent loop
//! consumes them as they arrive (token-by-token rendering, early
//! cancellation). The full response is the ordered concatenation of
//! every chunk plus a terminal [`Chunk::Done`].

pub mod http;

pub use http::HttpProvider;

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// The role attached to a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Developer-supplied directive that frames the conversation.
    System,
    /// Input from the human (or upstream caller).
    User,
    /// Output produced by the model.
    Assistant,
    /// Result of a tool call, fed back into the model.
    Tool,
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Who produced this message.
    pub role: Role,
    /// Textual content. May be empty when the message is purely a list
    /// of tool calls (assistant) or carries only a tool result.
    pub content: String,
    /// Tool calls produced by the assistant in this message, if any.
    //
    // `skip_serializing_if` keeps the wire payload compact: optional
    // fields aren't emitted when None, matching what providers expect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// If `role == Role::Tool`, the id of the call this message answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// A complete tool invocation produced by the assistant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Stable id assigned by the provider; we echo it on the matching
    /// tool response so the model can correlate.
    pub id: String,
    /// Tool name, e.g. `"read_file"`.
    pub name: String,
    /// Tool arguments as a JSON-encoded string. Each tool deserializes
    /// this into its own typed argument struct at dispatch time.
    pub arguments: String,
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural end-of-turn.
    Stop,
    /// Hit the max-tokens cap.
    Length,
    /// Stopped to invoke tools; the assistant message has `tool_calls`.
    ToolCalls,
    /// Provider-specific or unknown reason.
    Other,
}

/// One incremental update from a streaming completion.
//
// Modeled as a sum type rather than a struct with a bag of optional
// fields: the agent loop pattern-matches on each variant - no implicit
// "if content is Some, also check if tool_calls is Some" tangle.
#[derive(Debug, Clone)]
pub enum Chunk {
    /// A piece of text content from the assistant.
    Content(String),
    /// A partial tool call. Tool-call output streams in pieces (id and
    /// name first, then arguments JSON character by character) - the
    /// agent loop reassembles by [`ToolCallDelta::index`].
    ToolCallDelta(ToolCallDelta),
    /// Stream finished cleanly with the given reason.
    Done(FinishReason),
}

/// One slice of an in-progress tool call.
//
// The `index` field is how we know which call a delta belongs to when
// the model emits multiple parallel calls in a single turn. `id` and
// `name` are set on the first delta for an index; subsequent deltas
// only carry argument fragments.
#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    /// Position of this call within the assistant turn (0, 1, ...).
    pub index: u32,
    /// Set on the first delta for `index`. Stable across deltas.
    pub id: Option<String>,
    /// Set on the first delta for `index`; the tool name being invoked.
    pub name: Option<String>,
    /// Incremental JSON for the arguments. Concatenate across deltas
    /// at the same `index` to recover the full argument string.
    pub arguments: Option<String>,
}

/// Description of a tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name; must match the dispatcher's registry.
    pub name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON schema for the arguments object. Handed to the provider
    /// as-is; the provider ferries it to the model.
    pub parameters: serde_json::Value,
}

/// A single completion request.
//
// `messages` borrows from the caller's conversation history so building a
// request inside the agent's tool-iteration loop doesn't clone the whole
// transcript per iteration (that grows O(n) with session length). `tools`
// stays owned because the registry rebuilds schemas per call.
#[derive(Debug, Clone)]
pub struct CompletionRequest<'a> {
    /// Conversation history so far (oldest first).
    pub messages: &'a [Message],
    /// Tools the model is allowed to call this turn.
    pub tools: Vec<ToolSchema>,
    /// Model identifier; provider may ignore (e.g. local llama.cpp).
    pub model: String,
    /// Sampling temperature; `None` lets the provider pick a default.
    pub temperature: Option<f32>,
    /// Hard cap on output tokens; `None` for provider default.
    pub max_tokens: Option<u32>,
}

/// Boxed chunk stream returned by [`Provider::complete`].
//
// `Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>` is a heap-
// allocated, type-erased stream of `Result<Chunk>`s, pinned in memory
// and safe to send across threads. Pinning is required by
// `Stream::poll_next`; `Send` lets the multi-thread tokio runtime
// schedule it on any worker.
pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<Chunk>> + Send>>;

/// LLM provider - anything that can stream a chat completion.
//
// `#[async_trait]` lets `async fn` appear in trait definitions in a
// shape that's compatible with `Box<dyn Provider>`. Native async-fn-
// in-trait (stable since 1.75) doesn't yet cover every Send + object-
// safety case we want, so the macro is the standard escape hatch.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Submit a request and return a stream of incremental chunks.
    async fn complete<'a>(&self, req: CompletionRequest<'a>) -> Result<ChunkStream>;

    /// List model identifiers the provider can serve. OpenAI-
    /// compatible servers expose this via `GET /v1/models`; for
    /// local single-model llama-server it returns one entry; for
    /// multi-model proxies (llama-swap, ollama, vLLM) it returns
    /// the full list. Used by the `/model` slash command's picker
    /// UI; results are not cached at the trait level (the caller
    /// decides whether to cache).
    async fn list_models(&self) -> Result<Vec<String>>;
}
