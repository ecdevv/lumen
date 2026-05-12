//! Bottom status bar: one row with `tokens · contextual hint` on the
//! left and `model · cwd` info on the right.
//!
//! Token usage rides on the *left* so it sits under the user's
//! natural focus zone while typing - on wide terminals the right
//! edge is past peripheral vision. It renders at default weight
//! while everything around it is dim, so the eye anchors on it
//! without needing dedicated spatial isolation.
//!
//! The `idle` mode label is deliberately absent - the conversation
//! pane's inline streaming indicator is the canonical signal for
//! "model is generating," and approval-pending state is signaled by
//! the pin overlay + the hint-row flipping to navigation hints. A
//! bottom-bar "idle" segment would just clutter the resting state.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::app::{AppState, ArmedKey, pretty_path};
use super::fg;

/// Separator between status-bar segments.
pub(super) const STATUS_SEP: &str = " · ";

/// One-row status bar rendered at the very bottom of the frame.
/// Two-column layout: `tokens · contextual hint` LEFT-aligned and
/// `model · cwd` RIGHT-aligned. Tokens stand out via *styling*
/// (un-dimmed in a sea of dim siblings) rather than spatial
/// isolation.
pub(super) fn render_bottom_status(frame: &mut Frame, area: Rect, app: &AppState) {
    let cols = Layout::horizontal([
        Constraint::Min(0), // left half - hint
        Constraint::Min(0), // right half - info, right-aligned
    ])
    .split(area);

    frame.render_widget(Paragraph::new(bottom_hint_line(app)), cols[0]);
    frame.render_widget(
        Paragraph::new(bottom_info_line(app)).alignment(Alignment::Right),
        cols[1],
    );
}

/// Left-aligned hint row. Token usage is always the leading
/// element (un-dimmed so it pops as a reference anchor under the
/// user's natural focus zone), followed by a state-dependent hint:
///
/// 1. Approval pending -> navigation hints for the menu.
/// 2. Armed chord (Esc or Ctrl+C within [`super::super::app::ARM_TIMEOUT`])
///    -> "press X again to {clear,quit}" warning.
/// 3. Otherwise -> policy label + Shift+Tab discoverability nudge.
fn bottom_hint_line(app: &AppState) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut spans: Vec<Span<'static>> = vec![
        Span::raw("  "),
        Span::raw(format_token_usage(app)),
        Span::styled("  ·  ", dim),
    ];

    if app.pending_approval.is_some() {
        spans.extend([
            Span::styled("↑/↓", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" navigate  ", dim),
            Span::styled("Enter", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" confirm  ", dim),
            Span::styled("y/a/n", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" shortcuts  ", dim),
            Span::styled("Esc", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" reject", dim),
        ]);
    } else if let Some(armed) = app.armed_key() {
        // Wording reflects what the *second* tap will do at the
        // current input state. Esc-with-content clears (double-tap
        // clear); Ctrl+C never arms with content (single-tap clear),
        // so the only Ctrl+C arm path is empty->quit.
        let msg = match (armed, app.input_is_empty()) {
            (ArmedKey::Esc, false) => "press Esc again to clear",
            (ArmedKey::Esc, true) => "press Esc again to quit",
            (ArmedKey::CtrlC, _) => "press Ctrl+C again to quit",
        };
        spans.push(Span::styled(msg, fg(Color::Yellow, Modifier::BOLD)));
    } else {
        let policy = app.auto_apply();
        let policy_style = match policy {
            lumen_core::AutoApply::Never => dim,
            lumen_core::AutoApply::Safe => fg(Color::Yellow, Modifier::empty()),
        };
        spans.extend([
            Span::styled(policy.label(), policy_style),
            Span::styled("  ·  ", dim),
            Span::styled("Shift+Tab", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" to cycle modes", dim),
        ]);
    }

    Line::from(spans)
}

/// Right-aligned info line: `model · cwd`. All dim so it reads as
/// reference material. Token usage moved to the left-side hint
/// row (see [`bottom_hint_line`]) where it sits under the user's
/// natural focus zone while typing.
fn bottom_info_line(app: &AppState) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    Line::from(vec![
        Span::styled(app.cfg.provider.model.clone(), dim),
        Span::styled(STATUS_SEP, dim),
        Span::styled(pretty_path(&app.cwd), dim),
        Span::raw("  "), // right pad before frame edge
    ])
}

/// Format token usage for the bottom status bar. Returns
/// `&'static str` while we're stuck on the placeholder so no
/// per-frame allocation happens; when the TODO below lands, swap
/// the return type to `String` (the `Span::raw` call site is
/// generic over `Into<Cow<'a, str>>` and won't need to change).
//
// TODO(token-tracking): plumb actual usage through the provider
// response -> Agent -> AppState. Most OpenAI-compat servers report
// `usage: { prompt_tokens, completion_tokens, total_tokens }` on
// non-streaming responses; for streaming, the final chunk
// optionally carries `usage`. We'll need:
//   * provider/http.rs: parse `usage` from the final SSE chunk
//   * agent.rs: AgentEvent::TokenUsage { prompt, completion } event
//   * AppState: running totals + per-model context-window cap
//   * this function: format the live numbers
// For now: stable placeholder showing the eventual shape so the
// layout doesn't reflow when real numbers land.
fn format_token_usage(_app: &AppState) -> &'static str {
    "--/-- (0%)"
}
