//! TUI rendering: top-level [`render`] entry, shared style helpers,
//! selection extraction, and the per-pane submodules.
//!
//! Module layout:
//! * [`render`] - top-level draw entry, lays out the five vertical
//!   regions (conversation, approval pin, input, status, bottom pad)
//!   and dispatches to each region's renderer.
//! * [`layout`] - pane-size math, wrap utilities, geometry helpers.
//!   Shared with `tui/input.rs` so the renderer and the keystroke
//!   side use one wrap policy.
//! * [`conversation_pane`] - chat history, tool calls, notes,
//!   streaming spinner, markdown rendering.
//! * [`input_pane`] - prompt gutter + wrap-aware textarea rendering.
//! * [`status_bar`] - one-row bottom status bar.
//! * [`approval_panel`] - pinned approval region (header + menu)
//!   plus the diff body lines rendered inline in the conversation.
//! * [`help_modal`] - the `?` help overlay.
//!
//! `NO_COLOR` is honored via a process-global atomic flag set once at
//! startup by [`set_no_color`]: any non-empty value of the env var
//! disables foreground colors while preserving modifiers (bold, dim),
//! per the no-color.org spec.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};

use super::app::{AppState, Selection};

mod approval_panel;
mod conversation_pane;
mod floating_palette;
mod help_modal;
mod input_pane;
mod layout;
mod model_picker;
mod settings_modal;
mod slash_palette;
mod status_bar;

// External callers (`tui::input`, `tui::timeline`, `tui::mod`) reach
// for these as `render::X` - re-export to preserve that API after the
// internal module split.
pub use layout::{cursor_to_visual, visual_row_count, visual_to_logical};

// Test-only re-exports: the `tests` submodule does `use super::*;` and
// reaches for these. Gated on `cfg(test)` because the production build
// never references them at the `render::` level (each pane submodule
// imports its own helpers directly from `layout` / `approval` /
// `conversation`).
#[cfg(test)]
pub(super) use approval_panel::approval_pin_height;
#[cfg(test)]
pub(super) use conversation_pane::{SPINNER_FRAMES, format_tool_action, preview, short_args};
#[cfg(test)]
pub(super) use layout::{MAX_INPUT_ROWS, char_wrap, input_height, textarea_inner_width};

#[cfg(test)]
#[path = "../../tests/tui/render/mod.rs"]
mod tests;

/// Process-global NO_COLOR flag. Set once at startup by [`set_no_color`].
static NO_COLOR: AtomicBool = AtomicBool::new(false);

/// Configure NO_COLOR awareness. Call once before the first render.
pub fn set_no_color(value: bool) {
    NO_COLOR.store(value, Ordering::Relaxed);
}

pub(super) fn no_color() -> bool {
    NO_COLOR.load(Ordering::Relaxed)
}

/// Build a `Style` with the given foreground color, but strip the color
/// when NO_COLOR is set. Modifiers passed in are preserved.
pub(super) fn fg(color: Color, modifiers: Modifier) -> Style {
    let style = Style::default().add_modifier(modifiers);
    if no_color() {
        style
    } else {
        style.fg(color)
    }
}

/// Shared style for the soft-framing horizontal rules around the
/// input pane (TOP+BOTTOM) and the approval pin (TOP). Explicit
/// indexed gray rather than `Modifier::DIM` because:
///
/// * `DIM` is interpreted differently by terminals (some render it
///   as 50%-intensity, others as a hint they ignore entirely),
///   which made the two regions look uneven even though the
///   applied style was identical.
/// * An indexed palette entry resolves to a stable hex value on
///   every modern terminal/theme combo, so the two rules render
///   genuinely uniformly.
///
/// `Indexed(248)` sits between `DarkGray` (ANSI 8, ~50%) and `Gray`
/// (ANSI 7, ~75% / terminal default) - a noticeable but soft tone
/// that reads as framing without disappearing on dark themes.
///
/// Honors NO_COLOR via the existing `fg()` helper - falls back to
/// modifier-less default (terminal foreground) when the user has
/// opted out of color.
pub(super) fn soft_separator_style() -> Style {
    fg(Color::Indexed(248), Modifier::empty())
}

/// Replace control characters that desync ratatui's per-cell buffer from
/// what the terminal actually paints.
//
// The big offender is `\t`: ratatui counts it as one cell of width when
// laying out a `Line`, but the terminal interprets it as "advance to
// next tab stop" (typically 8 cells). That drift accumulates across the
// row, so every glyph after a tab lands at a different terminal column
// than ratatui thinks it occupies. The visible artifacts are corrupted
// glyphs (e.g. `1at[workspace]` where the source had `1\t[workspace]`)
// and stray characters at the right edge from wrap calculations made
// against ratatui's narrower view.
//
// Hits us through the `read` tool's output (`{:>6}\t{line}` line-number
// prefix). Sanitizing here catches it for any tool that returns tab- or
// control-char content, and also disarms ANSI escape sequences (`\x1b`
// is a control char) embedded in adversarial model output.
pub(super) fn sanitize_for_display(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Compact human-readable duration for tool timings and turn footers:
/// `12ms`, `3.4s`, `2m 13s`. Dropping decimals once we cross seconds
/// keeps the column stable.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = d.as_secs();
    if secs < 60 {
        return format!("{:.1}s", d.as_secs_f64());
    }
    let mins = secs / 60;
    let rem = secs % 60;
    format!("{mins}m {rem}s")
}

/// Top-level draw. Splits the frame into conversation / approval pin /
/// input / status / bottom-padding rows and dispatches to each pane.
//
// Takes `&mut AppState` because:
//   * `render_conversation` clamps `scroll_offset` to the computed
//     `bottom_anchor` (otherwise wheel-up at the top accumulates without
//     bound and creates a "stuck on scroll-down" feel).
//   * The input pane's surrounding `Block` carries the status info as
//     left/right titles, so we rebuild it each frame from current
//     AppState (mode, spinner-tick, esc-armed) and call `set_block` on
//     the textarea.
pub fn render(frame: &mut Frame, app: &mut AppState) {
    let frame_h = frame.area().height;
    let frame_w = frame.area().width;
    // Stash the inner textarea width so input.rs can do wrap-aware
    // Up/Down navigation against the same wrap policy this frame
    // used to lay out the pane. Updated every frame so a SIGWINCH
    // resize is reflected on the very next keystroke.
    app.render.last_textarea_width = layout::textarea_inner_width(frame_w);
    // Approval pin: a fixed-height region between the conversation
    // and the input pane that holds the header + menu while a tool
    // awaits a verdict. Pinned so the menu stays visible even if
    // the user scrolls the conversation pane to read context.
    // Height is 0 when no approval is in flight (no layout cost).
    let pin_height = app
        .pending_approval
        .as_ref()
        .map_or(0, |p| approval_panel::approval_pin_height(&p.kind));
    let v = Layout::vertical([
        Constraint::Min(0),                                                       // conversation
        Constraint::Length(pin_height),                                           // approval pin
        Constraint::Length(layout::input_height(app, frame_h, frame_w, pin_height)), // input pane
        Constraint::Length(1),                                                    // bottom status bar
        Constraint::Length(1),                                                    // bottom padding
    ])
    .split(frame.area());

    conversation_pane::render_conversation(frame, v[0], app);
    if let Some(pending) = app.pending_approval.as_ref() {
        approval_panel::render_approval_pin(frame, v[1], pending, &app.cwd, &app.cfg.ui);
    }
    input_pane::render_input(frame, v[2], app);
    status_bar::render_bottom_status(frame, v[3], app);
    // v[4] is bottom padding - intentionally left empty.

    // Selection highlight + clipboard extraction land AFTER widgets
    // paint so the buffer reflects the final pixel state. Selection is
    // `Copy`, so reading it doesn't conflict with the buffer borrow.
    if let Some(sel) = app.selection {
        apply_selection_highlight(frame.buffer_mut(), sel);
        if app.render.copy_pending {
            let extracted = extract_selection_text(frame.buffer_mut(), sel);
            tracing::debug!(
                bytes = extracted.len(),
                trimmed_empty = extracted.trim().is_empty(),
                "extracted selection text for clipboard"
            );
            app.render.clipboard_pending = Some(extracted);
            app.render.copy_pending = false;
        }
    } else {
        // Defensive: selection cleared between mouse-up and the next
        // render means there's nothing to copy. Don't leak a stale flag.
        app.render.copy_pending = false;
    }

    // Floating palettes (slash / model picker) draw after the
    // main layout so they overlay the conversation pane, but
    // before the help overlay so help would still appear on top
    // if both somehow co-existed. In practice slash + model
    // picker are mutually exclusive (executing /model closes the
    // slash palette before opening the picker).
    slash_palette::render_slash_palette(frame, v[2].y, app);
    model_picker::render_model_picker(frame, v[2].y, app);

    if app.show_help {
        help_modal::render_help_overlay(frame);
    }
    // Settings overlay renders alongside help (mutually exclusive
    // in practice - `/settings` doesn't auto-open help and vice
    // versa).
    settings_modal::render_settings_modal(frame, app);
}

/// Toggle `Modifier::REVERSED` on cells inside the selection rect that
/// actually carry content. Trailing whitespace per row is skipped, and
/// rows with no content at all (blank lines, padding rows) get no
/// highlight - matches what [`extract_selection_text`] copies and keeps
/// the visual selection from looking like a wall of inverted padding.
fn apply_selection_highlight(buf: &mut Buffer, sel: Selection) {
    let area = buf.area;
    let (start, end) = sel.normalized();
    for y in start.1..=end.1 {
        if y >= area.height {
            break;
        }
        // Find the rightmost non-whitespace cell in this row. Cells
        // with empty/space symbols are treated as blank padding.
        let last_content_x = (0..area.width).rev().find(|&x| {
            buf.cell(Position::new(x, y))
                .is_some_and(|c| !c.symbol().trim().is_empty())
        });
        let Some(last_content_x) = last_content_x else {
            // Entirely-blank row - no content to highlight.
            continue;
        };

        let x_start = if y == start.1 { start.0 } else { 0 };
        let raw_x_end = if y == end.1 {
            end.0.saturating_add(1).min(area.width)
        } else {
            area.width
        };
        // Cap to the row's last content cell + 1 so trailing padding
        // doesn't get inverted.
        let x_end = raw_x_end.min(last_content_x.saturating_add(1));
        if x_start >= x_end {
            continue;
        }
        for x in x_start..x_end {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
}

/// Extract the text inside the selection rect from the post-render
/// buffer with three layers of cleanup so the clipboard contents look
/// like the user *intended* to copy, not the padded pane content:
///
/// 1. **Trailing whitespace stripped per row** (drag past content end
///    doesn't trail a wall of spaces).
/// 2. **Leading/trailing blank rows dropped** (drag into padding rows
///    above/below content doesn't sandwich newlines).
/// 3. **Dedented by the leftmost line's indent**: find the smallest
///    leading-space count across non-blank rows and strip that amount
///    from every row. The leftmost-line indent represents the
///    selection's visual left edge; anything *beyond* it on other
///    rows is intentional content indentation (a nested bullet, a
///    code block's indented body) and survives the copy.
///
/// Returns an empty string when the selection covers only whitespace -
/// the caller skips OSC 52 in that case.
fn extract_selection_text(buf: &Buffer, sel: Selection) -> String {
    let area = buf.area;
    let (start, end) = sel.normalized();
    let mut rows: Vec<String> = Vec::new();
    for y in start.1..=end.1 {
        if y >= area.height {
            break;
        }
        let x_start = if y == start.1 { start.0 } else { 0 };
        let x_end = if y == end.1 {
            end.0.saturating_add(1).min(area.width)
        } else {
            area.width
        };
        let mut row = String::new();
        for x in x_start..x_end {
            if x >= area.width {
                break;
            }
            if let Some(cell) = buf.cell(Position::new(x, y)) {
                row.push_str(cell.symbol());
            }
        }
        rows.push(row.trim_end().to_string());
    }

    // Dedent: strip the leftmost-line's leading-space count from every
    // row. Leading whitespace in our buffer is always ASCII space
    // (block-padding cells default to ' '; body prefixes are literal
    // `"  "`), so char count == byte count and the byte-range slice
    // is safe.
    let min_indent = rows
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| r.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    if min_indent > 0 {
        for row in &mut rows {
            if !row.is_empty() {
                row.replace_range(..min_indent, "");
            }
        }
    }

    // Strip leading + trailing blank rows. Internal blanks survive
    // (paragraph breaks within a selection are intentional).
    let first = rows.iter().position(|r| !r.is_empty());
    let last = rows.iter().rposition(|r| !r.is_empty());
    match (first, last) {
        (Some(f), Some(l)) => rows[f..=l].join("\n"),
        _ => String::new(),
    }
}
