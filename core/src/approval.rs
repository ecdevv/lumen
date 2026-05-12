//! Approval gate for proposed file edits and shell commands.
//!
//! The framework asks an [`ApprovalGate`] to confirm side effects
//! before applying them. v0.1 ships only [`AutoAcceptGate`] (always
//! [`Verdict::Accept`]); the TUI-backed gate that shows a diff
//! overlay and waits for a keypress lands in step 10.
//!
//! Trait-on-context (not event-on-stream) for one reason: tools have
//! `&ToolContext` at hand but no direct line to the agent's event
//! callback. A trait method on the gate lets the *implementation*
//! pick its own channel mechanism (oneshot, mpsc, blocking, scripted
//! auto-reject for CI) without changing the tool-side API.

use std::path::Path;

use async_trait::async_trait;

/// Prefix that tool result strings use to mark a user rejection.
/// Stable across Write / Edit / Shell so the renderer can detect
/// rejection without coupling to any single tool's message format.
//
// The model also reads this prefix from the system prompt's
// "Tool results are authoritative" section, so changing this
// string requires updating both [`crate::CORE_SYSTEM_PROMPT`]
// and every tool that produces it.
pub const REJECTION_PREFIX: &str = "REJECTED by user:";

/// Outcome of one approval review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Proceed with the proposed change.
    Accept,
    /// Skip the proposed change. The tool surfaces a "cancelled by
    /// user" result back to the model so it can plan around the
    /// rejection rather than retry the same edit.
    Reject,
}

/// Approval gate for proposed side effects. Tools call into the gate
/// just before applying a change; the implementation decides how to
/// surface the decision.
//
// `Send + Sync` because the gate is `Arc`'d into [`crate::tools::ToolContext`]
// which is `Clone` and crosses task boundaries during dispatch.
// `Debug` because `ToolContext` derives Debug and the bound is
// required for the trait object to participate.
#[async_trait]
pub trait ApprovalGate: Send + Sync + std::fmt::Debug {
    /// Review a proposed file edit. `diff` is a unified-format string;
    /// `path` is the target file (already sandboxed by the caller).
    async fn review_diff(&self, path: &Path, diff: &str) -> Verdict;

    /// Review a proposed shell command. Called by the shell tool on
    /// every invocation regardless of [`crate::config::AutoApply`]
    /// mode - shell has no auto-accept tier in v0.1. Per-command
    /// allowlisting (a future `/allow <pattern>` slash command) is
    /// the path for trusted shells.
    async fn review_shell(&self, command: &str) -> Verdict;
}

/// No-op gate: every review returns [`Verdict::Accept`]. The v0.1
/// default - preserves pre-step-10 tool behavior (Write/Edit/Shell
/// run unconditionally) while the seam is in place for step 10 to
/// drop a real TUI-backed gate onto.
//
// Zero-sized type, trivially `Clone`/`Copy`. Construct with
// `AutoAcceptGate` (unit value) or `AutoAcceptGate::default()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoAcceptGate;

#[async_trait]
impl ApprovalGate for AutoAcceptGate {
    async fn review_diff(&self, _path: &Path, _diff: &str) -> Verdict {
        Verdict::Accept
    }
    async fn review_shell(&self, _command: &str) -> Verdict {
        Verdict::Accept
    }
}

/// Gate that rejects every review. Useful as a "dry-run" mode for
/// CI / scripted runs, and as a test double for verifying tools
/// short-circuit on rejection.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysRejectGate;

#[async_trait]
impl ApprovalGate for AlwaysRejectGate {
    async fn review_diff(&self, _path: &Path, _diff: &str) -> Verdict {
        Verdict::Reject
    }
    async fn review_shell(&self, _command: &str) -> Verdict {
        Verdict::Reject
    }
}

#[cfg(test)]
#[path = "tests/approval.rs"]
mod tests;
