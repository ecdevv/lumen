//! Conversation timeline model.
//!
//! The timeline is a flat ordered list of [`TimelineItem`]s the
//! renderer walks top-to-bottom. Streamed assistant text chunks
//! coalesce into the trailing assistant block so token-rate updates
//! don't shred history into hundreds of one-letter items.
//!
//! Tool calls land as their own item with a mutable [`ToolStatus`]
//! that flips Running -> Done | Error when the matching `ToolCallEnd`
//! event arrives. Lookup is by id, scanning from the end (the active
//! call is almost always the last ToolCall item).
//!
//! [`Timeline::apply`] dispatches an [`AgentEvent`] into the right
//! mutation: appending to the trailing assistant block, transitioning
//! a tool call's status, or pushing a fresh user/note item.

use std::time::{Duration, Instant};

use lumen_core::AgentEvent;

/// Lifecycle state of one tool dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    /// Dispatch is in flight - the spinner shows here in the renderer.
    Running,
    /// Completed successfully; carries the result text fed back to the model.
    Done(String),
    /// Dispatch failed; carries the error text.
    Error(String),
}

/// One block in the conversation timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineItem {
    /// The user's own input.
    User(String),
    /// Assistant text. Streamed chunks coalesce into the trailing item
    /// of this kind via [`Timeline::push_assistant_text`].
    AssistantText(String),
    /// Tool dispatch with mutable status.
    ToolCall {
        /// Provider-assigned id. Stable across the call's lifetime.
        id: String,
        /// Tool name as the model emitted it.
        name: String,
        /// Reassembled JSON argument string.
        arguments: String,
        /// Lifecycle state.
        status: ToolStatus,
        /// When the dispatch began. Used to compute `elapsed` once the
        /// status flips to Done/Error.
        started: Instant,
        /// Wall-clock time the dispatch took. `None` while running;
        /// `Some` once finish_tool_call lands.
        elapsed: Option<Duration>,
    },
    /// Free-form system note: cancellation, retry, transport error, etc.
    Note(String),
}

/// The conversation log the TUI renders.
#[derive(Debug, Default)]
pub struct Timeline {
    items: Vec<TimelineItem>,
    /// When the current turn began. Set on [`Timeline::push_user`],
    /// consumed on [`AgentEvent::TurnEnd`] to emit a "Cooked for X"
    /// footer note. `None` between turns; we don't emit a footer for
    /// turns that ended via cancellation or transport error since
    /// those land their own explanatory notes upstream.
    turn_started: Option<Instant>,
}

/// Pick a duration-keyed verb for the turn-end footer note. Adds a
/// little personality to longer waits without inventing extra UI.
fn turn_verb(d: Duration) -> &'static str {
    match d.as_secs() {
        0..=2 => "Replied",
        3..=14 => "Cooked",
        15..=59 => "Crunched",
        _ => "Brewed",
    }
}

impl Timeline {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// All items, oldest first.
    #[must_use]
    pub fn items(&self) -> &[TimelineItem] {
        &self.items
    }

    /// `true` if there are no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn push_user(&mut self, content: String) {
        self.items.push(TimelineItem::User(content));
        // Stamp turn-start; consumed on TurnEnd to print a footer.
        self.turn_started = Some(Instant::now());
    }

    /// Append a streamed text chunk to the trailing assistant block,
    /// or start a new block if the trailing item isn't assistant text.
    pub fn push_assistant_text(&mut self, chunk: String) {
        if let Some(TimelineItem::AssistantText(buf)) = self.items.last_mut() {
            buf.push_str(&chunk);
        } else {
            self.items.push(TimelineItem::AssistantText(chunk));
        }
    }

    pub fn push_tool_call(&mut self, id: String, name: String, arguments: String) {
        self.items.push(TimelineItem::ToolCall {
            id,
            name,
            arguments,
            status: ToolStatus::Running,
            started: Instant::now(),
            elapsed: None,
        });
    }

    /// Update a previously-started tool call by id with its result.
    /// No-op when the id can't be found - shouldn't happen in normal
    /// flow but we don't want a stray End event to crash the UI.
    pub fn finish_tool_call(&mut self, id: &str, result: String, is_error: bool) {
        // Reverse scan: the matching call is overwhelmingly the latest
        // ToolCall item (sequential dispatch within a turn).
        for item in self.items.iter_mut().rev() {
            if let TimelineItem::ToolCall {
                id: existing,
                status,
                started,
                elapsed,
                ..
            } = item
                && existing == id
            {
                *status = if is_error {
                    ToolStatus::Error(result)
                } else {
                    ToolStatus::Done(result)
                };
                *elapsed = Some(started.elapsed());
                return;
            }
        }
    }

    pub fn push_note(&mut self, content: String) {
        self.items.push(TimelineItem::Note(content));
    }

    /// Drop every item from the timeline.
    //
    // Reserved for the `/clear` slash command; allow until then so
    // `-D warnings` doesn't reject the build.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Apply one [`AgentEvent`] to the timeline. Returns `true` if the
    /// turn ended so the caller (the TUI event loop) can flip the app
    /// back into idle mode.
    pub fn apply(&mut self, event: AgentEvent) -> bool {
        match event {
            AgentEvent::AssistantText(t) => {
                self.push_assistant_text(t);
                false
            }
            AgentEvent::ToolCallStart {
                id,
                name,
                arguments,
            } => {
                self.push_tool_call(id, name, arguments);
                false
            }
            AgentEvent::ToolCallEnd {
                id,
                result,
                is_error,
            } => {
                self.finish_tool_call(&id, result, is_error);
                false
            }
            AgentEvent::TurnEnd { .. } => {
                if let Some(started) = self.turn_started.take() {
                    let elapsed = started.elapsed();
                    let footer = format!(
                        "{} for {}",
                        turn_verb(elapsed),
                        super::render::format_duration(elapsed),
                    );
                    self.items.push(TimelineItem::Note(footer));
                }
                true
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/tui/timeline.rs"]
mod tests;
