//! Approval pin rendering + diff styling.
//!
//! Layout: a pinned region between the conversation pane and the
//! input pane that holds the approval header + selectable menu while
//! a tool awaits a verdict. Diff *bodies* land in the conversation
//! pane (via [`approval_body_lines`]) so they scroll with the rest
//! of the history; the pin keeps the decision controls in a stable
//! screen position.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use super::super::app::{
    ApprovalKind, PendingApproval, approval_options, display_path,
};
use super::layout::PANE_HORIZONTAL_PAD;
use super::{fg, sanitize_for_display, soft_separator_style};

/// Total rows the approval pin will consume, including its top
/// border. Used by `render` to allocate the right layout slice.
//
// Layout per kind:
//   Diff: TOP border + header line + N option rows
//   Shell: TOP border + header line + command line + N option rows
//
// Where N comes from `approval_options(&kind).len()` (3 for diff,
// 2 for shell today).
pub fn approval_pin_height(kind: &ApprovalKind) -> u16 {
    let options = u16::try_from(approval_options(kind).len()).unwrap_or(0);
    let body_rows: u16 = match kind {
        ApprovalKind::Diff { .. } => 1,        // header
        ApprovalKind::Shell { .. } => 1 + 1,   // header + command line
    };
    // +1 for the Block::TOP border line.
    1 + body_rows + options
}

/// Render the pinned approval region: a single-rule top border,
/// then the kind-specific header (and shell command), then the
/// 3- or 2-option menu. The body of a diff stays in the
/// conversation pane above this; the pin keeps the decision
/// controls in a stable screen position.
pub(super) fn render_approval_pin(
    frame: &mut Frame,
    area: Rect,
    pending: &PendingApproval,
    cwd: &std::path::Path,
    ui: &lumen_core::UiConfig,
) {
    let block = Block::default()
        .borders(Borders::TOP)
        // Same helper as `build_input_block` so the rule renders
        // byte-for-byte identically to the input pane's borders.
        .border_style(soft_separator_style())
        .padding(Padding::horizontal(PANE_HORIZONTAL_PAD));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    match &pending.kind {
        ApprovalKind::Diff { path, .. } => {
            lines.push(approval_header_diff(path, cwd));
        }
        ApprovalKind::Shell { command } => {
            lines.push(approval_header_shell());
            lines.push(approval_shell_command_line(command));
        }
    }
    lines.extend(approval_menu_lines(pending, ui));

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

/// Body lines for the approval preview, rendered inside the
/// conversation pane below the existing tool-call timeline entry.
/// Returns an empty Vec when there's nothing to show in the
/// conversation (e.g. shell prompts - the command appears in the
/// pin, so the conversation doesn't duplicate it).
//
// Diff bodies can be hundreds of lines, so they stay in the
// (scrollable) conversation pane while the pin keeps the header +
// menu visible. Shell commands are single-line and fit in the pin.
pub(super) fn approval_body_lines(
    pending: &PendingApproval,
    cwd: &std::path::Path,
) -> Vec<Line<'static>> {
    match &pending.kind {
        ApprovalKind::Diff { diff, .. } => shorten_diff_paths(diff, cwd)
            .lines()
            // Sanitize before styling so a tab / ESC in the diff
            // can't desync ratatui's cell math (same hardening
            // applied throughout conversation rendering).
            .map(|raw| style_diff_line(&sanitize_for_display(raw)))
            .collect(),
        ApprovalKind::Shell { .. } => Vec::new(),
    }
}

/// Post-process a unified-diff string to shorten the `--- a/<path>`
/// and `+++ b/<path>` header lines via [`display_path`]. The stored
/// diff stays canonical (full absolute path); this is render-only.
/// Other lines pass through untouched.
fn shorten_diff_paths(diff: &str, cwd: &std::path::Path) -> String {
    let mut out = String::with_capacity(diff.len());
    for line in diff.split_inclusive('\n') {
        match shorten_diff_header(line, "--- a/", cwd)
            .or_else(|| shorten_diff_header(line, "+++ b/", cwd))
        {
            Some(s) => out.push_str(&s),
            None => out.push_str(line),
        }
    }
    out
}

/// If `line` starts with `prefix` (a `--- a/` or `+++ b/` header
/// marker), strip the prefix, shorten the path payload, and
/// reconstruct. Returns `None` for non-header lines so the caller
/// passes them through verbatim.
fn shorten_diff_header(line: &str, prefix: &str, cwd: &std::path::Path) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let (path_str, trailer) = match rest.find('\n') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let shortened = display_path(std::path::Path::new(path_str), cwd);
    Some(format!("{prefix}{shortened}{trailer}"))
}

fn approval_header_diff(path: &std::path::Path, cwd: &std::path::Path) -> Line<'static> {
    Line::from(vec![
        Span::styled("? ", fg(Color::Yellow, Modifier::BOLD)),
        Span::styled("Apply edit to ", Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(display_path(path, cwd), fg(Color::Cyan, Modifier::BOLD)),
        Span::raw("?"),
    ])
}

fn approval_header_shell() -> Line<'static> {
    Line::from(vec![
        Span::styled("? ", fg(Color::Yellow, Modifier::BOLD)),
        Span::styled("Run shell command?", Style::new().add_modifier(Modifier::BOLD)),
    ])
}

fn approval_shell_command_line(command: &str) -> Line<'static> {
    // 2-space indent under the "? " marker; yellow to match the
    // fenced-code / inline-code color used elsewhere ("this is code").
    Line::from(format!("  {}", sanitize_for_display(command)))
        .style(fg(Color::LightYellow, Modifier::empty()))
}

/// Render the kind-specific selectable menu. `pending.selected`
/// (driven by Up/Down in `input.rs`) decides which row gets the
/// selection marker + bold-cyan styling; the rest render dim.
/// Letter shortcut shown in parens next to each label so users
/// can either navigate or shortcut. Source of truth for option
/// list is [`approval_options`]. Selection glyph honors
/// [`UiConfig::unicode_glyphs`] - `❯` by default, `>` when the
/// user has opted out of unicode.
fn approval_menu_lines(
    pending: &PendingApproval,
    ui: &lumen_core::UiConfig,
) -> Vec<Line<'static>> {
    let selected_marker = if ui.unicode_glyphs { "❯ " } else { "> " };
    approval_options(&pending.kind)
        .iter()
        .enumerate()
        .map(|(i, (_, label, shortcut))| {
            let is_selected = i == pending.selected;
            let (marker, label_style) = if is_selected {
                (selected_marker, fg(Color::Cyan, Modifier::BOLD))
            } else {
                ("  ", Style::default().add_modifier(Modifier::DIM))
            };
            Line::from(vec![
                Span::styled(marker, fg(Color::Cyan, Modifier::BOLD)),
                Span::styled((*label).to_string(), label_style),
                Span::raw("    "), // breathing room before shortcut
                Span::styled(
                    format!("({shortcut})"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])
        })
        .collect()
}

/// Style one line of a unified diff. Prefixes (`+ - @ space`) drive
/// the foreground color; file headers (`---`, `+++`) render dim-bold.
/// All lines get a 2-space indent so the diff aligns with other body
/// content in the conversation pane.
fn style_diff_line(line: &str) -> Line<'static> {
    let text = format!("  {line}");
    if line.starts_with("---") || line.starts_with("+++") {
        Line::from(text).style(Style::new().add_modifier(Modifier::DIM | Modifier::BOLD))
    } else if line.starts_with("@@") {
        Line::from(text).style(fg(Color::Cyan, Modifier::DIM))
    } else if line.starts_with('+') {
        Line::from(text).style(fg(Color::Green, Modifier::empty()))
    } else if line.starts_with('-') {
        Line::from(text).style(fg(Color::Red, Modifier::empty()))
    } else {
        Line::from(text).style(Style::new().add_modifier(Modifier::DIM))
    }
}
