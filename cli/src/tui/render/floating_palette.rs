//! Shared rendering primitive for floating palette dropdowns.
//!
//! Used by the slash-command palette and the model picker (and any
//! future filter-and-pick UI). Anchored to the top edge of the
//! input pane and grows *upward*, overlaying the bottom rows of
//! the conversation pane (which is always scrollable, so nothing
//! important gets permanently hidden).
//!
//! Callers own:
//!   * the underlying state (selected index, query, fetched items)
//!   * building `ListItem`s from their domain data
//!   * the palette's title text
//!
//! This module owns:
//!   * geometry (height clamp, anchor math, edge cases for tiny terminals)
//!   * the Block / Clear / List / highlight chrome
//!   * the dim-on-empty placeholder when items is empty

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

/// Max rows the palette will grow to (border + content). Caps the
/// height on terminals where the conversation pane is tall - we
/// don't want a small palette to dominate the screen.
pub(super) const MAX_PALETTE_ROWS: u16 = 8;

/// Render a floating palette anchored above `input_pane_top`.
///
/// `items` is the caller's already-built row content. Pass an empty
/// `items` to get a "no matches" placeholder; pass non-empty items
/// with `selected = Some(i)` to highlight a row. `selected = None`
/// suppresses the highlight (useful for transient states like
/// "loading...").
///
/// `empty_message` is shown as a dim single-row placeholder when
/// `items` is empty - e.g. "no commands match", "loading models...".
pub(super) fn render_floating_palette(
    frame: &mut Frame,
    input_pane_top: u16,
    title: &str,
    items: Vec<ListItem<'static>>,
    selected: Option<usize>,
    empty_message: &str,
) {
    // Compute geometry. Saturating cast: in practice item counts are
    // small (handful for slash; up to ~50 for model lists), but we
    // clamp defensively in case any list ever grows past u16.
    let row_count = u16::try_from(items.len().max(1)).unwrap_or(u16::MAX);
    let desired_h = (row_count + 2).min(MAX_PALETTE_ROWS);
    let height = desired_h.min(input_pane_top);
    if height < 3 {
        // Not enough room above the input pane to draw a bordered
        // palette. Skip rather than render a broken stub; the
        // caller's state stays open so the palette reappears when
        // the user resizes the terminal.
        return;
    }
    let frame_area = frame.area();
    let area = Rect {
        x: frame_area.x,
        y: input_pane_top.saturating_sub(height),
        width: frame_area.width,
        height,
    };

    // Clear paints empty cells so the overlay covers whatever was
    // drawn behind it (conversation pane content).
    frame.render_widget(Clear, area);

    // Resolve the row content. Empty items -> single dim placeholder
    // row carrying `empty_message`. Caller-provided items pass
    // through verbatim.
    let (display_items, allow_highlight) = if items.is_empty() {
        let placeholder = vec![ListItem::new(Line::from(Span::styled(
            format!("  {empty_message}"),
            Style::new().add_modifier(Modifier::DIM),
        )))];
        (placeholder, false)
    } else {
        (items, true)
    };

    // Build the bordered block with the caller-supplied title.
    // Owned String for title so the Block can outlive `title: &str`.
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));

    // ListState carries the selection cursor; List auto-scrolls if
    // items overflow the visible region. We only set selection
    // when items are real (not the empty-message placeholder).
    let mut state = ListState::default();
    if allow_highlight {
        if let Some(idx) = selected {
            // Clamp defensively: caller might pass a stale index if
            // their filter shrunk between render frames.
            state.select(Some(idx.min(display_items.len() - 1)));
        }
    }

    let list = List::new(display_items)
        .block(block)
        .highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state);
}
