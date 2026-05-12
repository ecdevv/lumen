//! TUI-backed [`ApprovalGate`] implementation.
//!
//! Tools running on the agent task call `ctx.gate.review_*(...)`. This
//! impl turns each call into a [`UiMsg::ApprovalRequest`] over the
//! existing UI channel and awaits a one-shot reply that the UI sends
//! when the user presses y / n / Esc.
//!
//! Two failure modes both resolve to [`Verdict::Reject`]:
//! * Sending the `UiMsg` fails - the UI channel has been closed (the
//!   render loop exited). The user can't be prompted, so we treat
//!   the request as rejected.
//! * Awaiting the reply fails (sender dropped without sending) - the
//!   UI cleared `pending_approval` without producing a verdict (e.g.
//!   `cancel_turn` while the modal was up). Same fail-safe-closed
//!   policy: reject.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use async_trait::async_trait;
use lumen_core::{ApprovalGate, AutoApply, Verdict};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use super::app::{ApprovalKind, UiMsg};

/// Approval gate that routes review requests to the TUI modal flow.
//
// Cheaply clonable: holds one `UnboundedSender` (which is itself an
// `Arc` internally) and one `Arc<AtomicU8>` shared with
// `ToolContext` and `AppState`. Constructed once at startup.
#[derive(Debug, Clone)]
pub struct TuiApprovalGate {
    ui_tx: UnboundedSender<UiMsg>,
    /// Shared approval policy. Read before each diff prompt so a
    /// Shift+Tab toggle into `Safe` mode skips the modal flow and
    /// auto-accepts file edits on subsequent reviews without
    /// restarting the agent. Shell review is unaffected by mode.
    auto_apply: Arc<AtomicU8>,
}

impl TuiApprovalGate {
    /// Wire the gate to the UI message channel and the shared
    /// approval-policy cell.
    #[must_use]
    pub fn new(ui_tx: UnboundedSender<UiMsg>, auto_apply: Arc<AtomicU8>) -> Self {
        Self { ui_tx, auto_apply }
    }

    fn policy(&self) -> AutoApply {
        AutoApply::from_u8(self.auto_apply.load(Ordering::Relaxed))
    }

    /// Send `kind` to the UI and await the verdict. Either failure
    /// mode resolves to `Reject` (fail-safe-closed).
    async fn ask(&self, kind: ApprovalKind) -> Verdict {
        let (tx, rx) = oneshot::channel();
        if self
            .ui_tx
            .send(UiMsg::ApprovalRequest { kind, reply: tx })
            .is_err()
        {
            tracing::warn!("ui channel closed; defaulting approval to Reject");
            return Verdict::Reject;
        }
        rx.await.unwrap_or_else(|_| {
            tracing::warn!("approval reply sender dropped; defaulting to Reject");
            Verdict::Reject
        })
    }
}

#[async_trait]
impl ApprovalGate for TuiApprovalGate {
    async fn review_diff(&self, path: &Path, diff: &str) -> Verdict {
        // `Safe` bypasses the diff prompt entirely - the user has
        // opted in to auto-accept edits for this session via
        // Shift+Tab or "Accept all this session" or config.
        if self.policy() == AutoApply::Safe {
            return Verdict::Accept;
        }
        self.ask(ApprovalKind::Diff {
            path: path.to_path_buf(),
            diff: diff.to_string(),
        })
        .await
    }

    async fn review_shell(&self, command: &str) -> Verdict {
        // No mode auto-accepts shell. Both `Never` and `Safe` route
        // every command through this gate; per-command allowlisting
        // (a future `/allow <pattern>` slash command) is the path
        // for trusted shells.
        self.ask(ApprovalKind::Shell {
            command: command.to_string(),
        })
        .await
    }
}

#[cfg(test)]
#[path = "../tests/tui/approval.rs"]
mod tests;
