//! Agent loop - drives provider <-> tools <-> session for one user turn.
//!
//! One [`Agent::turn`] call:
//! 1. Pushes the user input as a [`Role::User`] message.
//! 2. Calls the provider, streams chunks into an in-flight assistant
//!    message (text + reassembled tool calls).
//! 3. Pushes the completed assistant message.
//! 4. If the model requested tool calls, dispatches each through the
//!    [`ToolRegistry`], pushes a [`Role::Tool`] result for each, and
//!    loops back to step 2.
//! 5. Returns the terminal [`FinishReason`] when the model stops without
//!    asking for more tools.
//!
//! Plan-and-Execute is degenerate currently: the model is the
//! orchestrator (same shape as Claude Code). The structural seams for
//! explicit plan / reflect / replan steps can layer on top later.
//!
//! # Streaming events
//! [`AgentEvent`]s flow to the caller via a `FnMut` callback so the TUI
//! can render token-by-token without waiting for the turn to complete.
//! Events arrive in conversation order; tool-call `Start`/`End` pairs
//! never interleave (tools dispatch sequentially within a single
//! assistant turn).
//!
//! # Safety cap
//! `AgentOptions::max_tool_iterations` bounds the assistant <-> tool
//! ping-pong inside one user turn. Hitting it returns `Err(Error::Tool)`
//! rather than looping forever - a confused model shouldn't burn the
//! day's quota.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::StreamExt;

use crate::error::{Error, Result};
use crate::provider::{
    Chunk, CompletionRequest, FinishReason, Message, Provider, Role, ToolCall,
};
use crate::session::Session;
use crate::tools::{ToolContext, ToolRegistry};

/// Framework-level system prompt: identity, cross-cutting tool guidance,
/// response style. Seeded into a fresh session via
/// [`Agent::seed_system_prompt`].
///
/// **Stability matters.** This const, the tool schemas, and the user's
/// first message together form the prefix that providers cache. Any
/// edit to this string invalidates every running session's cache; bump
/// it consciously, not casually. Future layers (project `AGENTS.md` /
/// `CLAUDE.md`, user `memory.md`) compose *after* this string, not
/// inside it, so the constant prefix stays stable across users and
/// projects.
//
// Deliberately kept short: small local models (the v0.1 target) follow
// terse, direct instructions more reliably than long ones. Cross-
// cutting only - per-tool semantics already live in each tool's
// `description()`. The leading `\` after the opening quote eats the
// first newline so the rendered string starts on its first line.
pub const CORE_SYSTEM_PROMPT: &str = "\
You are lumen, a local-first coding assistant running in a terminal.

Your tools are sandboxed to the current working directory. Full per-tool schemas accompany this prompt; use the descriptions there for argument shapes and individual semantics.

When working with code:
- Read a file before editing it; edits expect a verbatim match against the file's current bytes.
- Prefer the dedicated grep tool over running `grep` or `rg` through shell.
- Never fabricate file contents, line numbers, or tool output - call the appropriate tool instead.

Tool results are authoritative:
- If a tool returns text starting with `REJECTED` or containing `was NOT performed` / `was NOT applied` / `was NOT executed`, the operation did NOT occur. Report the refusal truthfully. Do not claim the action succeeded. Ask the user how to proceed.
- Treat every tool result string as the source of truth for what happened. Do not narrate completion that the result does not confirm.

Response style:
- Be concise. The user reads your output in a single terminal pane.
- Don't restate the request or apologize.
- Skip preamble like \"I'll help you with that\".
- Prefer running tools over asking permission, unless the user explicitly asked for a plan.
";

/// Runtime knobs for one [`Agent`].
#[derive(Debug, Clone)]
pub struct AgentOptions {
    /// Model identifier sent to the provider. Empty string is the
    /// "unset" sentinel - callers should set a real name before
    /// dispatching a turn. Some single-model `llama-server` builds
    /// ignore the field, but multi-model proxies (llama-swap, ollama,
    /// vLLM) route on it and stricter servers reject empty values.
    pub model: String,
    /// Sampling temperature; `None` lets the provider pick a default.
    pub temperature: Option<f32>,
    /// Hard cap on output tokens per chunked response; `None` for the
    /// provider's default.
    pub max_tokens: Option<u32>,
    /// Safety cap on the assistant <-> tool ping-pong inside one user
    /// turn. Most turns finish in 1-5 iterations; 50 is a "definitely
    /// runaway" threshold.
    pub max_tool_iterations: usize,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            model: String::new(),
            temperature: None,
            max_tokens: None,
            max_tool_iterations: 50,
        }
    }
}

/// Streaming event produced as a turn unfolds.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// One piece of assistant text content (token-level chunk).
    AssistantText(String),
    /// A tool call is about to dispatch.
    ToolCallStart {
        /// Provider-assigned id; matches the eventual `Role::Tool` message.
        id: String,
        /// Tool name as the model emitted it.
        name: String,
        /// Reassembled JSON argument string.
        arguments: String,
    },
    /// A tool call returned (success or error).
    //
    // `name` is intentionally not on this variant: consumers correlate
    // `End` with the matching `Start` by `id` and already have the
    // name from there. Carrying it twice would invite "what if id and
    // name disagree?" divergence with no value.
    ToolCallEnd {
        /// Same id as the matching [`AgentEvent::ToolCallStart`].
        id: String,
        /// Result text fed back to the model.
        result: String,
        /// `true` when dispatch failed and `result` is the error string.
        is_error: bool,
    },
    /// Model finished generating without requesting more tools.
    TurnEnd {
        /// Reason the model stopped.
        reason: FinishReason,
    },
}

/// Drives one or more user turns. Owns the conversation [`Session`], the
/// [`Provider`] to call, and the [`ToolRegistry`] / [`ToolContext`] used
/// to dispatch tool calls.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    session: Session,
    ctx: ToolContext,
    options: AgentOptions,
}

impl Agent {
    /// Construct an agent from its component parts. Caller is responsible
    /// for any system-prompt seeding via `session.push(...)` before the
    /// first turn.
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        session: Session,
        ctx: ToolContext,
        options: AgentOptions,
    ) -> Self {
        Self {
            provider,
            tools,
            session,
            ctx,
            options,
        }
    }

    /// Borrow the underlying session.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Mutably borrow the underlying session - for ad-hoc notes or
    /// custom message injection between turns. For the standard
    /// framework system prompt, prefer [`Self::seed_system_prompt`]
    /// which enforces the "first message, exactly once" invariant.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Update the model identifier used in subsequent requests.
    /// `AgentOptions` captures the model at construction; `/model`
    /// and `/settings` mutate the Config but the Agent needs to be
    /// told about it explicitly. Caller is responsible for not
    /// switching mid-turn (the next `turn()` picks it up).
    pub fn set_model(&mut self, model: String) {
        self.options.model = model;
    }

    /// Clone of the shared provider handle. Lets callers invoke
    /// provider methods (e.g. `list_models()`) without holding the
    /// agent's outer mutex - useful when the TUI wants to fetch
    /// metadata in parallel with a mid-flight turn.
    pub fn provider(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.provider)
    }

    /// Push `prompt` as the session's first message with [`Role::System`].
    ///
    /// **Idempotent on non-empty sessions.** Calling on a session that
    /// already has messages is a silent no-op. Two reasons:
    /// * Resumed sessions ([`Session::resume`]) carry their own system
    ///   prompt from the original transcript; re-seeding would duplicate
    ///   it and silently degrade prompt-cache hit rate on every turn.
    /// * Callers can defensively `seed_system_prompt` from any startup
    ///   path without first probing whether the session is fresh.
    ///
    /// Callers compose the prompt before this call - typically
    /// [`CORE_SYSTEM_PROMPT`] alone in v0.1; later phases concatenate
    /// project `AGENTS.md` / user `memory.md` content on top.
    //
    // Takes `&str` and clones into the message rather than `String` so
    // callers can pass `CORE_SYSTEM_PROMPT` directly without an `.into()`.
    pub async fn seed_system_prompt(&mut self, prompt: &str) -> Result<()> {
        if !self.session.messages().is_empty() {
            return Ok(());
        }
        self.session
            .push(Message {
                role: Role::System,
                content: prompt.to_string(),
                tool_calls: None,
                tool_call_id: None,
            })
            .await
    }

    /// Run one user turn to completion. Returns the model's terminal
    /// `FinishReason`. Streams events as they occur.
    pub async fn turn<F>(&mut self, user_input: String, mut on_event: F) -> Result<FinishReason>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.session
            .push(Message {
                role: Role::User,
                content: user_input,
                tool_calls: None,
                tool_call_id: None,
            })
            .await?;

        for _ in 0..self.options.max_tool_iterations {
            let stream = self.provider.complete(self.build_request()).await?;
            let (text, calls, finish_reason) = consume_stream(stream, &mut on_event).await?;

            let tool_calls_field = if calls.is_empty() {
                None
            } else {
                Some(calls.clone())
            };
            // Debug-level instrumentation: log the full coalesced
            // assistant message text with whitespace escaped (so `\n`
            // shows literally as `\n` in the log). Lets us diagnose
            // "is the model emitting walls of newlines" without
            // guessing. Enable with `LUMEN_LOG=debug` (or `RUST_LOG`).
            tracing::debug!(
                tool_call_count = calls.len(),
                content = %text.escape_debug(),
                "assistant message"
            );
            self.session
                .push(Message {
                    role: Role::Assistant,
                    content: text,
                    tool_calls: tool_calls_field,
                    tool_call_id: None,
                })
                .await?;

            if calls.is_empty() {
                on_event(AgentEvent::TurnEnd {
                    reason: finish_reason,
                });
                return Ok(finish_reason);
            }

            self.dispatch_tool_calls(calls, &mut on_event).await?;
        }

        Err(Error::Tool(format!(
            "agent exceeded max_tool_iterations ({})",
            self.options.max_tool_iterations
        )))
    }

    fn build_request(&self) -> CompletionRequest<'_> {
        // Borrows from the session - the wire layer copies only what it
        // serializes, so we avoid cloning the entire transcript per
        // tool-iteration loop.
        CompletionRequest {
            messages: self.session.messages(),
            tools: self.tools.schemas(),
            model: self.options.model.clone(),
            temperature: self.options.temperature,
            max_tokens: self.options.max_tokens,
        }
    }

    async fn dispatch_tool_calls<F>(
        &mut self,
        calls: Vec<ToolCall>,
        on_event: &mut F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        for call in calls {
            on_event(AgentEvent::ToolCallStart {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });

            let (result, is_error) = match self
                .tools
                .invoke(&self.ctx, &call.name, &call.arguments)
                .await
            {
                Ok(s) => (s, false),
                // Stringify the Error and feed it back as the tool
                // result - the model can recover (e.g. retry with a
                // valid path). Anything that should abort the turn
                // would have propagated via `?` upstream; everything
                // reaching here is model-recoverable.
                Err(e) => (e.to_string(), true),
            };

            on_event(AgentEvent::ToolCallEnd {
                id: call.id.clone(),
                result: result.clone(),
                is_error,
            });

            self.session
                .push(Message {
                    role: Role::Tool,
                    content: result,
                    tool_calls: None,
                    tool_call_id: Some(call.id),
                })
                .await?;
        }
        Ok(())
    }
}

/// Drain one streaming completion: emit text events, accumulate tool-
/// call deltas keyed by `index`, return the assembled text + ordered
/// tool calls + terminal `FinishReason`.
//
// Free function rather than a method because it doesn't touch `self` -
// keeping it standalone makes turn()'s control flow obvious and lets
// future Plan-and-Execute layers reuse the streaming loop without
// pulling in the whole Agent.
async fn consume_stream<F>(
    mut stream: crate::provider::ChunkStream,
    on_event: &mut F,
) -> Result<(String, Vec<ToolCall>, FinishReason)>
where
    F: FnMut(AgentEvent) + Send,
{
    let mut text = String::new();
    // Tool-call deltas arrive in pieces keyed on `index`; we accumulate
    // per-index until the stream's `Done` chunk tells us the assistant
    // turn is structurally complete.
    let mut acc: BTreeMap<u32, PartialCall> = BTreeMap::new();
    let mut finish_reason = FinishReason::Other;

    while let Some(chunk) = stream.next().await {
        match chunk? {
            Chunk::Content(t) => {
                text.push_str(&t);
                emit_text_chunks(&t, on_event);
            }
            Chunk::ToolCallDelta(d) => {
                let entry = acc.entry(d.index).or_default();
                if let Some(id) = d.id {
                    entry.id = id;
                }
                if let Some(name) = d.name {
                    entry.name = name;
                }
                if let Some(args) = d.arguments {
                    entry.arguments.push_str(&args);
                }
            }
            Chunk::Done(reason) => {
                finish_reason = reason;
                break;
            }
        }
    }

    // `into_values` consumes the BTreeMap in key order, giving tool
    // calls back in the index order the model emitted them.
    let calls: Vec<ToolCall> = acc
        .into_values()
        .map(|p| ToolCall {
            id: p.id,
            name: p.name,
            arguments: p.arguments,
        })
        .collect();

    Ok((text, calls, finish_reason))
}

/// Accumulator for one streaming tool call (one ChunkStream `index`).
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Hard cap on one `AgentEvent::AssistantText` payload. Bounded so a
/// pathologically large provider chunk (or an adversarial response)
/// can't put a multi-MB `String` through the UI channel in a single
/// event. Typical streaming chunks are 1-500 bytes; 8 KiB sits well
/// above that ceiling and well below where memory pressure matters.
const MAX_TEXT_CHUNK: usize = 8 * 1024;

/// Split `text` into `AgentEvent::AssistantText` events, none larger
/// than [`MAX_TEXT_CHUNK`] bytes. Splits at UTF-8 char boundaries so
/// we never emit a half-codepoint string.
//
// Free function (not method) for the same reason as `consume_stream`:
// it doesn't touch agent state, keeps the streaming loop's control
// flow obvious, and is trivially testable in isolation.
fn emit_text_chunks<F>(text: &str, on_event: &mut F)
where
    F: FnMut(AgentEvent),
{
    if text.is_empty() {
        return;
    }
    if text.len() <= MAX_TEXT_CHUNK {
        on_event(AgentEvent::AssistantText(text.to_string()));
        return;
    }
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MAX_TEXT_CHUNK).min(text.len());
        // UTF-8 chars are at most 4 bytes, so the boundary is at most
        // 3 bytes behind `end`. Walk back to one.
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        // `start` is always a previous `end` (or 0), which was a char
        // boundary; so `end == start` only if the slice is empty,
        // which the loop condition already excluded. Defensive bail
        // in case of an unexpected edge.
        debug_assert!(end > start, "emit_text_chunks failed to advance");
        on_event(AgentEvent::AssistantText(text[start..end].to_string()));
        start = end;
    }
}

#[cfg(test)]
#[path = "tests/agent.rs"]
mod tests;
