//! Slash-command palette dropdown. Thin caller over
//! [`super::floating_palette::render_floating_palette`] - this
//! module just builds the row items from [`SlashCommand`]s and
//! decides when to render.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;

use super::super::app::AppState;
use super::super::slash::{SlashCommand, filter_commands, is_slash_query};
use super::fg;
use super::floating_palette::render_floating_palette;

/// Render the slash palette if it's open. `input_pane_top` is the
/// y-coordinate of the input pane's top edge. No-op when the
/// palette is closed.
pub(super) fn render_slash_palette(
    frame: &mut Frame,
    input_pane_top: u16,
    app: &AppState,
) {
    let Some(palette) = app.slash_palette.as_ref() else {
        return;
    };
    let lines = app.input.lines();
    if !is_slash_query(lines) {
        return; // sync_slash_palette should already have closed it, but be safe.
    }
    let matches = filter_commands(&lines[0]);
    let items: Vec<ListItem<'static>> = matches
        .iter()
        .map(|c| ListItem::new(command_row(c)))
        .collect();
    let selected = (!matches.is_empty()).then_some(palette.selected);
    render_floating_palette(
        frame,
        input_pane_top,
        "Commands",
        items,
        selected,
        "no commands match",
    );
}

/// One row in the palette: `/<name>` cyan-bold, two spaces, dim
/// description. Format mirrors the help modal's keybind rows for
/// visual consistency.
fn command_row(c: &SlashCommand) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("/{:<10}", c.name), fg(Color::Cyan, Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(c.description, Style::new().add_modifier(Modifier::DIM)),
    ])
}
