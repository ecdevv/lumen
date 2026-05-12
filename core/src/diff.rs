//! Unified-diff construction for tool-side approval flows.
//!
//! Write/Edit build a diff between the file's current bytes and the
//! proposed bytes, hand it to the [`crate::approval::ApprovalGate`],
//! and apply only on [`crate::approval::Verdict::Accept`]. The diff
//! is also what the TUI renders inside the approval modal.
//!
//! Backed by `similar::TextDiff`. Single-file scope: each diff is one
//! `--- a/<path>` / `+++ b/<path>` pair followed by zero or more
//! hunks. Multi-file refactors aggregate per-tool-call.

use std::path::Path;

use similar::TextDiff;

/// Default context lines per hunk. `similar` uses 3 by default; we
/// match. Three is enough for a human to anchor each hunk in the
/// surrounding code without burying the change.
const CONTEXT_LINES: usize = 3;

/// Build a unified diff from `old` to `new` for `path`. Returns an
/// empty string when the inputs are byte-identical (callers treat that
/// as "no change, skip approval and write").
///
/// Header convention: `--- a/<path>` / `+++ b/<path>`. New-file edits
/// (where `old` is empty) keep the same shape rather than emitting
/// `/dev/null` - the resulting diff is all `+` lines and the path on
/// both sides reads as the target.
#[must_use]
pub fn unified_diff(old: &str, new: &str, path: &Path) -> String {
    if old == new {
        return String::new();
    }
    let path_str = path.display().to_string();
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .header(&format!("a/{path_str}"), &format!("b/{path_str}"))
        .to_string()
}

#[cfg(test)]
#[path = "tests/diff.rs"]
mod tests;
