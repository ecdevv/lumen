//! Slash-command palette: command registry, filter logic, state,
//! key dispatch, and action execution. Everything `/`-related lives
//! here so a contributor can understand the feature in one file.
//!
//! The palette opens when the user types `/` on an empty single-line
//! input, filters [`COMMANDS`] live as they type, and runs the
//! highlighted command on Enter. Rendering: see
//! `render/slash_palette.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lumen_core::CORE_SYSTEM_PROMPT;

use super::app::{Action, AppState};
use super::input::is_ctrl_d;
use super::model_picker;
use super::settings::SettingsState;
use super::timeline::Timeline;

/// What a slash command does when executed. Dispatch lives in
/// [`execute_action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashAction {
    /// Open the help overlay.
    Help,
    /// Exit lumen. Identical to Ctrl+D.
    Quit,
    /// Reset the UI timeline and the agent's in-memory message
    /// history. The on-disk transcript is preserved.
    Clear,
    /// Switch model (`/model <name>`) or open the picker
    /// submenu (bare `/model`).
    Model,
    /// Open the settings overlay - a catalog of editable config
    /// fields with type-aware edit interactions (Text / Bool /
    /// Enum). Commits write through to the config file.
    Settings,
}

/// One entry in the slash palette. Static so the [`COMMANDS`] table
/// is `const`-buildable and zero-allocation per render.
#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    /// Command name *without* the leading `/`. The palette displays
    /// it with the slash prefixed for visual consistency with the
    /// user's typed query.
    pub name: &'static str,
    /// Short, one-line description shown to the right of the name.
    pub description: &'static str,
    pub action: SlashAction,
}

/// All slash commands available in v0.1. Order is the palette's
/// default display order when the query is bare `/` (no filter).
///
/// **Invariant:** every `name` is lowercase ASCII so
/// [`filter_commands`] can prefix-match a lowercased query against
/// the raw name without an extra allocation per command. The
/// `command_names_are_lowercase_ascii` test pins this.
pub const COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "help",
        description: "Show keybindings",
        action: SlashAction::Help,
    },
    SlashCommand {
        name: "clear",
        description: "Clear conversation history",
        action: SlashAction::Clear,
    },
    SlashCommand {
        name: "model",
        description: "Switch model (/model <name>) or open picker",
        action: SlashAction::Model,
    },
    SlashCommand {
        name: "settings",
        description: "Open settings (view & edit configuration)",
        action: SlashAction::Settings,
    },
    SlashCommand {
        name: "quit",
        description: "Exit lumen",
        action: SlashAction::Quit,
    },
];

/// Active slash-palette state. `None` on [`super::app::AppState`]
/// means the palette is closed and `/` behaves as a literal char.
#[derive(Debug, Clone)]
pub struct SlashPalette {
    /// Index into the current `matches` of the highlighted row.
    /// Clamped on every filter recompute so it never points past
    /// the end. 0 = first row by default.
    pub selected: usize,
}

impl SlashPalette {
    pub fn new() -> Self {
        Self { selected: 0 }
    }
}

/// Filter [`COMMANDS`] against the user's query. The query is
/// expected to start with `/`; the leading slash is stripped before
/// matching, then we case-insensitively prefix-match each
/// command's `name`. Bare `/` (empty after stripping) returns all
/// commands in registration order - the palette renders the full
/// list as a discoverable menu.
///
/// Returns a `Vec<&SlashCommand>` so callers can index directly
/// into the borrowed static entries without re-allocating strings.
pub fn filter_commands(query: &str) -> Vec<&'static SlashCommand> {
    let needle = query.strip_prefix('/').unwrap_or(query);
    // Match against the first whitespace-delimited token only;
    // anything after a space is treated as inline command arguments
    // (extracted via `parse_command_args` at execute time) and
    // doesn't participate in filtering. Lets the user type
    // `/model gpt-4o` with `/model` still highlighted.
    let cmd_token = needle
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if cmd_token.is_empty() {
        return COMMANDS.iter().collect();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(cmd_token.as_str()))
        .collect()
}

/// Extract the inline argument string from a slash query - i.e.
/// everything after the first whitespace following the command
/// name, trimmed. Returns the empty string when no args are
/// present.
///
/// Examples:
///   * `"/model gpt-4o"` -> `"gpt-4o"`
///   * `"/model   gpt-4o  "` -> `"gpt-4o"` (trimmed)
///   * `"/model"` -> `""`
pub fn parse_command_args(query: &str) -> &str {
    let needle = query.strip_prefix('/').unwrap_or(query);
    needle
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest)
        .trim()
}

/// `true` when the input buffer is in "slash mode" - a single
/// non-empty line beginning with `/`. The palette stays open
/// exactly when this is true (plus the explicit `slash_palette`
/// flag, which the user can clear with Esc).
pub fn is_slash_query(lines: &[String]) -> bool {
    lines.len() == 1 && lines[0].starts_with('/')
}

// ----- Key dispatch + action execution -------------------------- //

/// Dispatch one keystroke while the slash palette is open. Returns
/// `Some(action)` for keys the palette claims (nav, commit,
/// dismiss, force-quit); `None` falls through so the main match
/// handles editing keystrokes (typing / backspace / undo) and the
/// post-pass [`sync_palette`] reconciles state.
pub(super) fn handle_palette_key(k: KeyEvent, app: &mut AppState) -> Option<Action> {
    app.slash_palette.as_ref()?;
    if is_ctrl_d(k) {
        return Some(Action::Quit);
    }
    match (k.code, k.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            close_palette(app);
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
            Some(execute_palette(app))
        }
        _ => None,
    }
}

/// Close the slash palette and clear the input. Used by Esc/Ctrl+C
/// inside the palette and as the cleanup step after every command
/// executes (the leading `/` and any filter text become moot once
/// the command has run).
pub(super) fn close_palette(app: &mut AppState) {
    app.slash_palette = None;
    app.reset_input();
}

/// Move the highlighted row up by one. Saturates at 0; no-op when
/// the palette is closed.
fn move_selection_up(app: &mut AppState) {
    if let Some(palette) = app.slash_palette.as_mut() {
        palette.selected = palette.selected.saturating_sub(1);
    }
}

/// Move the highlighted row down by one, clamped to the last live
/// match. No-op when the palette is closed or the filter yields
/// no matches.
fn move_selection_down(app: &mut AppState) {
    let lines = app.input.lines();
    if !is_slash_query(lines) {
        return;
    }
    let max = filter_commands(&lines[0]).len().saturating_sub(1);
    if let Some(palette) = app.slash_palette.as_mut() {
        palette.selected = (palette.selected + 1).min(max);
    }
}

/// Run the highlighted command. If the filter yielded no matches
/// (e.g. the user typed `/blarg`), this is a no-op - the palette
/// stays open and the user can fix the typo or Esc out.
fn execute_palette(app: &mut AppState) -> Action {
    let lines = app.input.lines();
    if !is_slash_query(lines) {
        close_palette(app);
        return Action::Continue;
    }
    let query = lines[0].clone();
    let matches = filter_commands(&query);
    let selected = app.slash_palette.as_ref().map_or(0, |p| p.selected);
    let Some(cmd) = matches.get(selected) else {
        return Action::Continue;
    };
    let args = parse_command_args(&query).to_string();
    execute_action(app, cmd.action, &args)
}

/// Dispatch a [`SlashAction`] to its side effect. `args` carries
/// inline arguments parsed from the input buffer (the text after
/// the command name + whitespace). Every branch calls
/// [`close_palette`] first so the palette is dismissed before
/// the command's effect lands.
fn execute_action(app: &mut AppState, action: SlashAction, args: &str) -> Action {
    close_palette(app);
    match action {
        SlashAction::Help => {
            app.show_help = true;
            Action::Continue
        }
        SlashAction::Quit => Action::Quit,
        SlashAction::Clear => clear_session(app),
        SlashAction::Model => model_picker::execute_model_command(app, args),
        SlashAction::Settings => {
            app.settings = Some(SettingsState::new());
            Action::Continue
        }
    }
}

/// Wipe the UI timeline and reset the agent's in-memory message
/// history to just the framework system prompt. On-disk transcript
/// is preserved. `try_lock` instead of blocking: during a mid-flight
/// turn the spawned task holds the mutex. Failing fast with a note
/// is honest - the user can cancel the turn (Esc) and retry.
/// Locking inline would deadlock the TUI thread.
//
// `reset_to_system_prompt` (not `clear_messages`) so the next turn
// keeps the tool-use guidance from CORE_SYSTEM_PROMPT and the byte-
// stable prompt-cache prefix from DESIGN.md. A bare `clear_messages`
// would ship an empty system prompt on the next turn and bust the
// cache for the rest of the session.
fn clear_session(app: &mut AppState) -> Action {
    if let Ok(mut agent) = app.agent.try_lock() {
        agent
            .session_mut()
            .reset_to_system_prompt(CORE_SYSTEM_PROMPT);
        app.timeline = Timeline::new();
        app.timeline.push_note("conversation cleared".into());
    } else {
        app.timeline
            .push_note("can't clear while a turn is running".into());
    }
    Action::Continue
}

/// Reconcile palette state with the current input contents. Called
/// at the end of every keystroke that reaches the main match - so
/// the palette closes when the user edits away the leading `/`, and
/// the selected row stays clamped to the live filter result. No-op
/// when the palette is already closed.
pub(super) fn sync_palette(app: &mut AppState) {
    if app.slash_palette.is_none() {
        return;
    }
    let lines = app.input.lines();
    if !is_slash_query(lines) {
        app.slash_palette = None;
        return;
    }
    // `saturating_sub(1)` collapses both the empty-matches case
    // (max becomes 0; selected.min(0) = 0) and the populated case
    // (max = len - 1) into a single clamp.
    let max_selected = filter_commands(&lines[0]).len().saturating_sub(1);
    if let Some(palette) = app.slash_palette.as_mut() {
        palette.selected = palette.selected.min(max_selected);
    }
}

#[cfg(test)]
#[path = "../tests/tui/slash.rs"]
mod tests;
