//! Input pane rendering: prompt gutter, textarea, wrap-aware scrolling.
//!
//! Replaces `tui_textarea`'s built-in render path so long lines wrap
//! at the pane edge instead of scrolling horizontally. The textarea
//! is still the source of truth for editing state (cursor, history,
//! undo, keystroke routing); this module just reads `.lines()` /
//! `.cursor()` and paints the wrapped result.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use super::super::app::AppState;
use super::layout::{
    INPUT_BLOCK_HPAD, PROMPT_GUTTER_WIDTH, char_wrap, cursor_to_visual,
};
use super::soft_separator_style;

/// Default placeholder when the textarea is empty and no
/// floating palette is active. Inline-rendered here (not via
/// `tui_textarea::set_placeholder_text`) because we replaced
/// the textarea's own render path with our wrap-aware version.
const INPUT_PLACEHOLDER: &str =
    "type a message · Enter to send · Shift/Alt+Enter for newline";

/// Placeholder when the model picker is open. The input buffer
/// doubles as a filter string in that state, so the hint reflects
/// the filter-and-pick interaction rather than message-send.
const MODEL_PICKER_PLACEHOLDER: &str =
    "type to filter models · Enter to switch · Esc to cancel";

/// Pick the placeholder string for the current app state. Slash
/// palette never coincides with an empty input (the `/` trigger
/// keeps at least one char in the buffer), so it has no entry
/// here.
fn placeholder_for(app: &AppState) -> &'static str {
    if app.model_picker.is_some() {
        MODEL_PICKER_PLACEHOLDER
    } else {
        INPUT_PLACEHOLDER
    }
}

pub(super) fn render_input(frame: &mut Frame, area: Rect, app: &AppState) {
    // Render the surrounding block (TOP + BOTTOM borders) first.
    // Inner is the writable area inside the borders + horizontal padding.
    let block = build_input_block();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner into a 2-col prompt gutter + the rest for the
    // textarea content. The prompt sits on the first row only;
    // continuation rows (multi-line OR wrap-induced) get blank
    // gutter to match shell feel.
    let cols = Layout::horizontal([
        Constraint::Length(PROMPT_GUTTER_WIDTH),
        Constraint::Min(0),
    ])
    .split(inner);

    let prompt = Paragraph::new(Line::from(Span::styled(
        ">",
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(prompt, cols[0]);

    // Custom render path replaces `tui_textarea`'s built-in
    // rendering so we can wrap long lines at the pane edge instead
    // of scrolling them horizontally. The textarea is still the
    // source of truth for editing state (cursor, history, undo,
    // keystroke routing) - we just read .lines() and .cursor()
    // here and paint the wrapped result ourselves.
    let width = usize::from(cols[1].width);
    let logical_lines = app.input.lines();
    let is_empty_input = logical_lines.len() == 1 && logical_lines[0].is_empty();

    if is_empty_input {
        // Placeholder lives entirely in our render path now.
        let placeholder = Paragraph::new(Line::from(Span::styled(
            placeholder_for(app),
            Style::new().add_modifier(Modifier::DIM),
        )));
        frame.render_widget(placeholder, cols[1]);
        frame.set_cursor_position(Position::new(cols[1].x, cols[1].y));
        return;
    }

    let mut visual: Vec<Line<'static>> = Vec::new();
    for logical in logical_lines {
        for wrapped in char_wrap(logical, width) {
            visual.push(Line::from(wrapped));
        }
    }

    let (logical_row, logical_col) = app.input.cursor();
    let (cursor_vrow, cursor_vcol) =
        cursor_to_visual(logical_lines, logical_row, logical_col, width);
    let cursor_vrow_usize = usize::from(cursor_vrow);

    // Vertical scroll within the pane: when wrapped content
    // exceeds the pane's content area (capped by `input_height`),
    // we'd otherwise paint past the bottom border and the cursor
    // would escape the pane entirely. Anchor the cursor at the
    // bottom-most visible row by computing a scroll offset that
    // brings `cur_vrow` to exactly `pane_height - 1`. For
    // not-overflowing content the offset is 0 (saturating sub).
    //
    // This is "follow-the-cursor" scrolling, not soft-scroll: as
    // the user types past the pane height, the view tracks the
    // typing cursor. Up-arrow navigation also drags the view
    // upward to keep the cursor on screen.
    let pane_height = usize::from(cols[1].height).max(1);
    let scroll = cursor_vrow_usize.saturating_sub(pane_height.saturating_sub(1));
    let scroll_u16 = u16::try_from(scroll).unwrap_or(u16::MAX);

    frame.render_widget(
        Paragraph::new(visual).scroll((scroll_u16, 0)),
        cols[1],
    );

    let adjusted_vrow_usize = cursor_vrow_usize.saturating_sub(scroll);
    let adjusted_vrow = u16::try_from(adjusted_vrow_usize).unwrap_or(u16::MAX);
    frame.set_cursor_position(Position::new(
        cols[1].x.saturating_add(cursor_vcol),
        cols[1].y.saturating_add(adjusted_vrow),
    ));
}

/// Build the input pane's surrounding `Block`. TOP + BOTTOM borders
/// bracket the input as a discrete region; no title, no left gutter.
/// Matches the classic terminal-UI shape (vim command line, mc panel
/// footer): two horizontal rules with the editable content between.
//
// `padding(horizontal(2))` puts the prompt `>` at frame column 2,
// matching the conversation-pane body indent, the approval pin's
// content column, and the bottom status row's leading pad. Every
// piece of body content sits on the same vertical guideline.
fn build_input_block() -> Block<'static> {
    Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(soft_separator_style())
        .padding(Padding::horizontal(INPUT_BLOCK_HPAD))
}
