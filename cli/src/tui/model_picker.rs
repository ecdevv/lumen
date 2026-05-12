//! Model picker state, key dispatch, and the model-switching
//! logic shared between `/model <name>`, the bare `/model` picker
//! flow, and the `/settings` model field. Everything related to
//! "what model is lumen talking to" lives here.
//!
//! The picker reuses the slash-palette's "input buffer doubles as
//! search field" pattern: when the picker is open, typing into the
//! input filters the model list. Up/Down navigate, Enter switches.
//! Rendering: see `render/model_picker.rs`.
//!
//! State transitions:
//!   * `None` -> `Some(Loading)` when `/model` (no args) opens the picker
//!   * `Loading` -> `Loaded` when `UiMsg::ModelsLoaded(Ok(_))` arrives
//!   * `Loading` -> `Error` when `UiMsg::ModelsLoaded(Err(_))` arrives
//!   * Any -> `None` when Esc/Ctrl+C dismisses or a model is selected

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{Action, AppState, UiMsg};
use super::input::is_ctrl_d;
use super::settings::{ApplyError, Field};

/// Phase of the picker's async fetch lifecycle.
#[derive(Debug, Clone)]
pub enum ModelPickerStatus {
    /// Fetch is in flight. Picker renders a "loading models..."
    /// placeholder; no selection is highlighted.
    Loading,
    /// Fetch succeeded. `models` holds the full list returned by
    /// the provider; the filter narrows live as the user types.
    Loaded { models: Vec<String> },
    /// Fetch failed. `message` carries a short reason for display.
    /// User can Esc out and retry.
    Error { message: String },
}

/// Active model-picker state. `None` on [`super::app::AppState`]
/// means the picker is closed.
#[derive(Debug, Clone)]
pub struct ModelPickerState {
    /// Index into the current `matches` of the highlighted row.
    /// Clamped on every filter recompute. 0 by default.
    pub selected: usize,
    pub status: ModelPickerStatus,
}

impl ModelPickerState {
    pub fn loading() -> Self {
        Self {
            selected: 0,
            status: ModelPickerStatus::Loading,
        }
    }
}

/// Filter the loaded model list by a substring needle (case-
/// insensitive). When the needle is empty, returns all models in
/// the order the provider gave them. When the picker is in
/// `Loading` or `Error` state, returns an empty list.
///
/// Substring (not prefix) match matches the way users think about
/// model names - they'll type "qwen" to find "qwen2.5-coder-32b",
/// which is the middle of the canonical name on some providers.
pub fn filter_models<'a>(status: &'a ModelPickerStatus, needle: &str) -> Vec<&'a str> {
    let ModelPickerStatus::Loaded { models } = status else {
        return Vec::new();
    };
    if needle.is_empty() {
        return models.iter().map(String::as_str).collect();
    }
    let needle_lower = needle.to_ascii_lowercase();
    models
        .iter()
        .filter(|m| m.to_ascii_lowercase().contains(&needle_lower))
        .map(String::as_str)
        .collect()
}

// ----- Slash-side entry: /model command --------------------------- //

/// Run the `/model` slash command. Two modes:
///   * With inline args (`/model <name>`) - switch the live model
///     identifier and persist immediately. Cheap; no async work.
///   * Without args - open the picker (async fetch of available
///     models via `Provider::list_models()`).
pub(super) fn execute_model_command(app: &mut AppState, args: &str) -> Action {
    if !args.is_empty() {
        switch_model(app, args);
        return Action::Continue;
    }
    open_picker(app);
    Action::Continue
}

/// Swap the in-memory model identifier and announce the change.
/// Delegates the validate-mutate-persist work to
/// [`Field::apply_and_persist`] (same canonical path as
/// `/settings`); wraps with /model-specific note text.
pub(super) fn switch_model(app: &mut AppState, name: &str) {
    if app.cfg.provider.model == name {
        app.timeline
            .push_note(format!("model already set to {name}"));
        return;
    }
    let result = Field::ProviderModel.apply_and_persist(
        &mut app.cfg,
        app.cfg_path.as_deref(),
        name,
    );
    match result {
        Ok(()) => {
            app.timeline.push_note(format!("model switched to {name}"));
        }
        Err(ApplyError::Validation(msg)) => {
            // Field::ProviderModel doesn't validate today, but
            // future tightening (URL parse, allowlist) lands here.
            app.timeline.push_note(format!("model error: {msg}"));
        }
        Err(ApplyError::Persist(msg)) => {
            tracing::warn!(error = %msg, "persist /model failed");
            app.timeline.push_note(format!(
                "model switched to {name} (in-memory only - couldn't write config: {msg})"
            ));
        }
        Err(ApplyError::NoConfigPath) => {
            app.timeline.push_note(format!(
                "model switched to {name} (in-memory only - no config file path)"
            ));
        }
    }
}

// ----- Picker open / close --------------------------------------- //

/// Open the picker. If we have a cached model list from a prior
/// `/model` open, populate immediately - the cache is session-
/// scoped and avoids a refetch round-trip. Otherwise drop into
/// Loading state and spawn an async fetch task.
///
/// The fetch grabs a clone of the `Arc<dyn Provider>` from the
/// agent (held briefly to clone, dropped before the HTTP call) so
/// a mid-flight turn doesn't block the fetch and vice versa. On
/// completion, the task sends [`UiMsg::ModelsLoaded`]; if the user
/// has already closed the picker by then, the message is silently
/// dropped in `AppState::apply_ui_msg` (but the cache still warms).
pub(super) fn open_picker(app: &mut AppState) {
    if let Some(models) = app.cached_models.clone() {
        app.model_picker = Some(ModelPickerState {
            selected: 0,
            status: ModelPickerStatus::Loaded { models },
        });
        return;
    }
    app.model_picker = Some(ModelPickerState::loading());
    let agent = Arc::clone(&app.agent);
    let tx = app.agent_tx.clone();
    tokio::spawn(async move {
        let provider = {
            let guard = agent.lock().await;
            guard.provider()
        };
        let result = provider.list_models().await.map_err(|e| e.to_string());
        let _ = tx.send(UiMsg::ModelsLoaded(result));
    });
}

/// Close the picker and clear the input buffer (which was
/// doubling as the filter). Mirrors `slash::close_palette`.
fn close_picker(app: &mut AppState) {
    app.model_picker = None;
    app.reset_input();
}

// ----- Key dispatch ---------------------------------------------- //

/// Dispatch one keystroke while the picker is open. Same shape as
/// `slash::handle_palette_key`: claim nav/commit/dismiss keys;
/// let typing fall through to the textarea so the input buffer
/// can filter.
pub(super) fn handle_picker_key(k: KeyEvent, app: &mut AppState) -> Option<Action> {
    app.model_picker.as_ref()?;
    if is_ctrl_d(k) {
        return Some(Action::Quit);
    }
    match (k.code, k.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            close_picker(app);
            Some(Action::Continue)
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            move_selection_up(app);
            Some(Action::Continue)
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            move_selection_down(app);
            Some(Action::Continue)
        }
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            commit_picker(app);
            Some(Action::Continue)
        }
        _ => None,
    }
}

fn move_selection_up(app: &mut AppState) {
    if let Some(picker) = app.model_picker.as_mut() {
        picker.selected = picker.selected.saturating_sub(1);
    }
}

fn move_selection_down(app: &mut AppState) {
    let max = {
        let Some(picker) = app.model_picker.as_ref() else {
            return;
        };
        let needle = app.input.lines().first().map_or("", String::as_str);
        let count = filter_models(&picker.status, needle).len();
        if count == 0 {
            return;
        }
        count - 1
    };
    if let Some(picker) = app.model_picker.as_mut() {
        picker.selected = (picker.selected + 1).min(max);
    }
}

/// Commit the highlighted row: switch the live model identifier
/// and close the picker. No-op when the picker is in
/// loading/error state or the filter yields no matches.
fn commit_picker(app: &mut AppState) {
    let chosen = {
        let Some(picker) = app.model_picker.as_ref() else {
            return;
        };
        let needle = app.input.lines().first().map_or("", String::as_str);
        let matches = filter_models(&picker.status, needle);
        matches.get(picker.selected).map(ToString::to_string)
    };
    let Some(name) = chosen else {
        return;
    };
    close_picker(app);
    switch_model(app, &name);
}

/// Clamp the picker's `selected` to the live filter result.
/// Called from the main `handle_key` post-pass so user typing
/// (which shrinks the match list) doesn't leave the highlight
/// pointing past the end.
pub(super) fn sync_picker(app: &mut AppState) {
    let Some(picker) = app.model_picker.as_ref() else {
        return;
    };
    let needle = app.input.lines().first().map_or("", String::as_str);
    let max = filter_models(&picker.status, needle).len().saturating_sub(1);
    if let Some(picker) = app.model_picker.as_mut() {
        picker.selected = picker.selected.min(max);
    }
}

#[cfg(test)]
#[path = "../tests/tui/model_picker.rs"]
mod tests;
