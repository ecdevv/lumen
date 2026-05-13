//! Settings modal overlay: centered, content-sized, sectioned
//! field list with a highlighted selection. Reuses the help
//! overlay's centered-content-sized geometry pattern.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use super::super::app::AppState;
use super::super::settings::{EditBuffer, Field, FieldKind, SettingsState};
use super::fg;

/// Horizontal padding inside the modal border (matches help_modal).
const H_PAD: u16 = 2;
const BORDER: u16 = 1;
const TITLE_PAD: u16 = 2;

/// Width of the label column (left of each row). Tuned to fit
/// the widest current label (`auto_copy_on_select` = 19 chars)
/// with breathing room.
const LABEL_COL_WIDTH: usize = 22;

/// Render the settings overlay if it's open. No-op otherwise.
pub(super) fn render_settings_modal(frame: &mut Frame, app: &AppState) {
    let Some(state) = app.settings.as_ref() else {
        return;
    };

    let lines = build_lines(state, &app.cfg);
    let content_w = u16::try_from(lines.iter().map(Line::width).max().unwrap_or(40))
        .unwrap_or(u16::MAX);
    let content_h = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let desired_w = content_w + 2 * (BORDER + H_PAD) + TITLE_PAD;
    let desired_h = content_h + 2 * BORDER;
    let frame_area = frame.area();
    let width = desired_w.min(frame_area.width);
    let height = desired_h.min(frame_area.height);
    let area = Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Settings ")
        .padding(Padding::new(H_PAD, H_PAD, 0, 0));

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}

/// Minimum value column width so very short values still leave
/// room for the hint marker to feel like its own column.
const MIN_VALUE_COL_WIDTH: usize = 10;

/// Build the full content lines for the modal: sectioned field
/// rows + a trailing keybinding hint row. Each field row is
/// either a "nav-mode" view (label + value + hint of how to
/// interact) or, for the selected Text field while editing, an
/// "edit-mode" view (label + edit buffer with a visible cursor).
fn build_lines(
    state: &SettingsState,
    cfg: &lumen_core::Config,
) -> Vec<Line<'static>> {
    // Pre-compute the value column width as the longest *display*
    // value across all fields. Padding every row to this width
    // lines up the `[edit]` / `[toggle]` / `[cycle]` hints in a
    // single vertical column. Floor at MIN_VALUE_COL_WIDTH so
    // short bool / enum values don't crowd the hint.
    let value_col_width = Field::ALL
        .iter()
        .map(|&f| display_value(f, cfg).len())
        .max()
        .unwrap_or(0)
        .max(MIN_VALUE_COL_WIDTH);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_section: Option<&'static str> = None;

    for (i, &field) in Field::ALL.iter().enumerate() {
        // Insert section header when crossing a boundary.
        if Some(field.section()) != current_section {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                field.section(),
                fg(Color::Cyan, Modifier::BOLD | Modifier::DIM),
            )));
            current_section = Some(field.section());
        }

        let selected = i == state.selected;
        let editing = selected && state.editing.is_some();
        lines.push(row_line(
            field,
            cfg,
            selected,
            editing,
            state.editing.as_ref(),
            value_col_width,
        ));
    }

    lines.push(Line::from(""));
    lines.push(footer_hint(state));
    lines
}

/// What the value column should render for a given field. Mirrors
/// the masking rules in `row_line` so the width computation in
/// `build_lines` and the actual render agree exactly on string
/// length (otherwise hints could wobble by a few cols depending
/// on whether masking is in effect).
fn display_value(field: Field, cfg: &lumen_core::Config) -> String {
    let raw = field.read(cfg);
    if field.sensitive() && !raw.is_empty() {
        "<redacted>".to_string()
    } else if raw.is_empty() {
        // Same sentinel for every empty field so model and api_key
        // read consistently when both are at their "unset" default.
        "<none>".to_string()
    } else {
        raw
    }
}

/// One field row. Format:
///
///   `> <label>   <value>    [edit / toggle / cycle hint]`
///
/// Edit-mode rows show the buffer content with a `█` cursor
/// glyph at the end instead of the static value + hint.
fn row_line(
    field: Field,
    cfg: &lumen_core::Config,
    selected: bool,
    editing: bool,
    edit: Option<&EditBuffer>,
    value_col_width: usize,
) -> Line<'static> {
    let cursor_prefix = if selected { "> " } else { "  " };
    let label = format!("{:<width$}", field.label(), width = LABEL_COL_WIDTH);

    // Labels share the help-modal "key column" style: Cyan +
    // Bold regardless of selection. Selection is communicated by
    // a row-wide background highlight (see Line::style below),
    // matching the slash / model picker conventions.
    let label_style = fg(Color::Cyan, Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = vec![
        Span::raw(cursor_prefix.to_string()),
        Span::styled(label, label_style),
    ];

    if editing {
        let buf = edit.map_or("", |e| e.buffer.as_str());
        // Edit buffer uses the same Cyan-Bold as labels so the
        // active text and the field it belongs to read as a unit.
        // (Sensitive fields show typed chars during edit so users
        // can verify input; masking only applies in nav mode.)
        spans.push(Span::styled(
            buf.to_string(),
            fg(Color::Cyan, Modifier::BOLD),
        ));
        // Visible cursor glyph at end of buffer.
        spans.push(Span::styled("█", fg(Color::Cyan, Modifier::BOLD)));
    } else {
        let display = display_value(field, cfg);
        // Pad the value to `value_col_width` so the hint column
        // lines up across rows regardless of value length.
        // Saturating sub: values longer than the precomputed
        // width get zero padding and push the hint right.
        let pad_count = value_col_width.saturating_sub(display.len());
        // Value column uses the default text style (mirrors the
        // help modal's "description" column) - selection is the
        // row background, not a per-span weight bump.
        spans.push(Span::raw(display));
        if pad_count > 0 {
            spans.push(Span::raw(" ".repeat(pad_count)));
        }
        spans.push(Span::raw("   "));
        spans.push(Span::styled(
            interaction_hint(field),
            Style::new().add_modifier(Modifier::DIM),
        ));
    }

    // Selection highlight: dark-gray row background, same as the
    // floating palettes use for their highlighted row. Applied
    // via Line::style so it spans the full row width (per-span
    // bg would only cover painted cells, not trailing padding).
    let line = Line::from(spans);
    if selected {
        line.style(Style::new().bg(Color::DarkGray))
    } else {
        line
    }
}

/// Per-kind label for what Enter does on this row.
fn interaction_hint(field: Field) -> &'static str {
    match field.kind() {
        FieldKind::Text => "[edit]",
        FieldKind::Bool => "[toggle]",
        FieldKind::Enum { .. } => "[cycle]",
    }
}

/// Bottom row of the modal. Changes between nav mode and edit
/// mode so the user always sees the relevant keybindings.
fn footer_hint(state: &SettingsState) -> Line<'static> {
    let dim = Style::default().dim();
    if state.editing.is_some() {
        Line::from(vec![
            Span::styled("Enter", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" commit  ", dim),
            Span::styled("Esc", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" cancel edit", dim),
        ])
    } else {
        Line::from(vec![
            Span::styled("↑/↓", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" navigate  ", dim),
            Span::styled("Enter", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" edit/toggle/cycle  ", dim),
            Span::styled("Esc", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" close", dim),
        ])
    }
}
