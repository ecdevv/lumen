//! Conversation session: in-memory message log + append-only JSONL transcript.
//!
//! A [`Session`] owns the running history of [`Message`]s for one
//! conversation. Each push is mirrored to a JSONL file on disk so the
//! conversation is replayable, diffable, and survives a crash. The
//! transcript is one event per line; [`TranscriptEvent::Message`] holds
//! a chat message and [`TranscriptEvent::Note`] holds a free-form meta
//! event (session-start, model-swap, error annotation, etc.).
//!
//! Files live at `<data_dir>/sessions/<uuid>.jsonl`. UUID v4 filenames
//! make concurrent agents collision-free. The writer flushes after every
//! event - `kill -9` keeps everything up to the last completed `push` /
//! `note`. Stronger durability (`sync_all`) is deferred; the kernel
//! buffer is enough to survive normal process exit.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::fs::{File, OpenOptions, create_dir_all};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::provider::{Message, Role};

/// Stable identifier for a session.
//
// Wrapper around `Uuid` so the public type is opaque - callers can't
// confuse a session id with some other UUID, and we keep room to swap
// the underlying representation later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generate a fresh v4 UUID-backed id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// One transcript event. Stored as a tagged JSON object so future
/// variants (tool start/end, model swap, ...) can be added without
/// breaking parse of old transcripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptEvent {
    /// A chat message - what the model produced or what we fed it.
    Message(Message),
    /// Free-form meta annotation (session-start, model-change, etc.).
    Note {
        /// Human-readable description.
        text: String,
    },
}

/// One JSONL line as written. `ts` is RFC-3339 UTC.
//
// `#[serde(flatten)]` on the event inlines its fields beside `ts`,
// keeping the on-disk shape flat:
// `{"ts":"...","kind":"message","role":"user","content":"hi"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TranscriptLine {
    ts: String,
    #[serde(flatten)]
    event: TranscriptEvent,
}

/// Append-only JSONL writer for one session's transcript.
#[derive(Debug)]
struct TranscriptWriter {
    path: PathBuf,
    file: File,
}

impl TranscriptWriter {
    async fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent).await?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        Ok(Self { path, file })
    }

    async fn write_event(&mut self, event: TranscriptEvent) -> Result<()> {
        let line = TranscriptLine {
            ts: now_rfc3339(),
            event,
        };
        let mut json = serde_json::to_string(&line)?;
        json.push('\n');
        self.file.write_all(json.as_bytes()).await?;
        // Flush every event: a crash mid-turn shouldn't lose history.
        self.file.flush().await?;
        Ok(())
    }
}

/// One conversation: an ordered list of [`Message`]s plus an optional
/// JSONL transcript that mirrors every push.
#[derive(Debug)]
pub struct Session {
    id: SessionId,
    messages: Vec<Message>,
    transcript: Option<TranscriptWriter>,
}

impl Session {
    /// Create a new session backed by a JSONL file under `data_dir`.
    /// Path: `<data_dir>/sessions/<id>.jsonl`. Parent dirs are created.
    pub async fn create(data_dir: &Path) -> Result<Self> {
        let id = SessionId::new();
        let path = data_dir.join("sessions").join(format!("{id}.jsonl"));
        let writer = TranscriptWriter::open(path).await?;
        Ok(Self {
            id,
            messages: Vec::new(),
            transcript: Some(writer),
        })
    }

    /// In-memory-only session; nothing is written to disk. Useful for
    /// tests and ephemeral one-shot invocations.
    #[must_use]
    pub fn ephemeral() -> Self {
        Self {
            id: SessionId::new(),
            messages: Vec::new(),
            transcript: None,
        }
    }

    /// Resume an existing session from its JSONL transcript. Replays
    /// every `Message` event into [`Session::messages`]; subsequent
    /// pushes append to the same file.
    pub async fn resume(path: &Path) -> Result<Self> {
        let bytes = tokio::fs::read(path).await?;
        // Lossy decode: a bit-rot byte in an old transcript shouldn't
        // make the whole file unreadable.
        let text = String::from_utf8_lossy(&bytes);
        let mut messages = Vec::new();
        for raw in text.lines() {
            if raw.trim().is_empty() {
                continue;
            }
            // Skip-don't-fail on torn / malformed lines. A `kill -9`
            // mid-flush leaves the tail line truncated; failing the
            // whole replay would permanently brick the transcript for
            // one unfinished write. Log it loudly so the user can see
            // why an old turn is missing.
            let line: TranscriptLine = match serde_json::from_str(raw) {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        line = %raw.chars().take(120).collect::<String>(),
                        "skipping malformed transcript line during resume",
                    );
                    continue;
                }
            };
            if let TranscriptEvent::Message(m) = line.event {
                messages.push(m);
            }
        }

        let id = id_from_path(path)?;
        let writer = TranscriptWriter::open(path.to_path_buf()).await?;
        Ok(Self {
            id,
            messages,
            transcript: Some(writer),
        })
    }

    /// Append a message to the conversation and (if non-ephemeral) the
    /// transcript. Order is preserved across crashes.
    //
    // In-memory push is authoritative; the transcript is best-effort
    // mirror-for-replay. A disk write failure must NOT abort the turn
    // - that would leave the wire conversation half-applied (e.g. an
    // Assistant(tool_calls) message with no matching Tool reply), and
    // the next turn would resend a malformed transcript that
    // OpenAI-compat servers reject. Warn-log and continue instead.
    pub async fn push(&mut self, msg: Message) -> Result<()> {
        if let Some(t) = self.transcript.as_mut()
            && let Err(e) = t.write_event(TranscriptEvent::Message(msg.clone())).await
        {
            tracing::warn!(error = %e, "transcript write failed; continuing in-memory only");
        }
        self.messages.push(msg);
        Ok(())
    }

    /// Record a meta event. No-op for ephemeral sessions. Same
    /// transcript-failure policy as [`Self::push`].
    pub async fn note(&mut self, text: impl Into<String>) -> Result<()> {
        if let Some(t) = self.transcript.as_mut()
            && let Err(e) = t.write_event(TranscriptEvent::Note { text: text.into() }).await
        {
            tracing::warn!(error = %e, "transcript note write failed");
        }
        Ok(())
    }

    /// All messages so far, oldest first.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Drop the in-memory message log. The on-disk transcript (if any)
    /// is **not** truncated - existing JSONL lines stay, and subsequent
    /// `push` / `note` calls continue appending. Low-level primitive;
    /// the `/clear` flow uses [`Self::reset_to_system_prompt`] instead
    /// so the framework system prompt + prompt-cache prefix survive.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Replace the in-memory message log with a single [`Role::System`]
    /// message containing `prompt`. The on-disk transcript is **not**
    /// touched - the original seeded prompt remains at the head of the
    /// JSONL, and subsequent `push` / `note` calls append after the
    /// pre-clear history. This is what `/clear` calls: a full visible
    /// reset for the user, but the next provider request still ships
    /// the framework's tool-use guidance and matches the cached prefix
    /// from prior turns.
    //
    // Sync (no transcript write) so the slash dispatch chain doesn't
    // have to cascade `async fn` through 6 call layers for a single
    // in-memory mutation.
    pub fn reset_to_system_prompt(&mut self, prompt: &str) {
        self.messages.clear();
        self.messages.push(Message {
            role: Role::System,
            content: prompt.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    /// Stable id for this session.
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// On-disk path for the transcript, if any.
    #[must_use]
    pub fn transcript_path(&self) -> Option<&Path> {
        self.transcript.as_ref().map(|t| t.path.as_path())
    }
}

fn id_from_path(p: &Path) -> Result<SessionId> {
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::Tool(format!("invalid transcript path: {}", p.display())))?;
    let uuid = Uuid::parse_str(stem)
        .map_err(|e| Error::Tool(format!("transcript filename is not a uuid: {e}")))?;
    Ok(SessionId(uuid))
}

fn now_rfc3339() -> String {
    // `format(&Rfc3339)` only fails on writer errors against the in-mem
    // String, which is unreachable; `unwrap_or_default` keeps this
    // panic-free without hiding a real bug.
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
