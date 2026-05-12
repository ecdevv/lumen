//! Model-picker dropdown. Same shape as the slash palette, just
//! with a different content source (provider model list). Calls
//! the shared [`super::floating_palette::render_floating_palette`]
//! primitive.

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;

use super::super::app::AppState;
use super::super::model_picker::{ModelPickerStatus, filter_models};
use super::fg;

/// Render the model picker if it's open. `input_pane_top` is the
/// y-coordinate of the input pane's top edge. No-op when the
/// picker is closed.
pub(super) fn render_model_picker(
    frame: &mut Frame,
    input_pane_top: u16,
    app: &AppState,
) {
    let Some(picker) = app.model_picker.as_ref() else {
        return;
    };

    // The input buffer doubles as a substring filter while the
    // picker is open. When the picker first opens the buffer is
    // empty (cleared by the /model executor), so the unfiltered
    // list shows.
    let needle = app
        .input
        .lines()
        .first()
        .map_or("", String::as_str);

    let (items, selected, empty_msg) = match &picker.status {
        ModelPickerStatus::Loading => (
            Vec::<ListItem<'static>>::new(),
            None,
            "loading models...",
        ),
        ModelPickerStatus::Error { message } => {
            let msg_owned: String = format!("error: {message}");
            // Render the error message as a fake "no items" state.
            // Caller passes empty items + dynamic empty_msg via a
            // detour - we leak a 'static reference via Box::leak
            // is overkill; instead we render the error inline as a
            // single item and keep selection None.
            let item = ListItem::new(Line::from(Span::styled(
                msg_owned,
                fg(Color::Red, Modifier::BOLD),
            )));
            (vec![item], None, "")
        }
        ModelPickerStatus::Loaded { .. } => {
            let matches = filter_models(&picker.status, needle);
            let selected = (!matches.is_empty()).then_some(picker.selected);
            let items: Vec<ListItem<'static>> = matches
                .iter()
                .map(|m| ListItem::new(model_row(m, &app.cfg.provider.model)))
                .collect();
            (items, selected, "no models match")
        }
    };

    super::floating_palette::render_floating_palette(
        frame,
        input_pane_top,
        "Models",
        items,
        selected,
        empty_msg,
    );
}

/// One row in the picker: model name, with the *currently active*
/// model marked with a `(current)` tag so the user can see at a
/// glance which one is in use.
fn model_row(name: &str, current: &str) -> Line<'static> {
    let mut spans = vec![Span::styled(
        name.to_string(),
        fg(Color::Cyan, Modifier::BOLD),
    )];
    if name == current {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "(current)",
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}
