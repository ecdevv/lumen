//! Settings modal: a catalog of editable config fields rendered
//! as a centered overlay (same chrome as the help modal). Opened
//! by the `/settings` slash command.
//!
//! Each [`Field`] variant carries enough metadata to:
//!   * label itself in the UI
//!   * read its current value from [`Config`]
//!   * apply a user-edited value back to `Config`
//!   * serialize that value to TOML for surgical writeback
//!
//! Why an enum instead of a runtime-built `Vec<FieldDef>` with
//! function pointers / boxed closures: the field set is fixed at
//! compile time; an enum lets the type system enforce
//! exhaustiveness in every match (display, read, apply, validate,
//! to_toml) so adding a new field surfaces every site that must
//! be updated as a compile error.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lumen_core::{AutoApply, Config, toml_edit};

use super::app::{Action, AppState};
use super::input::is_ctrl_d;

/// All editable fields, in display order. Each variant maps to
/// exactly one row in the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    ProviderModel,
    ProviderBaseUrl,
    ProviderApiKey,
    AutoApply,
    UiAutoCopyOnSelect,
    UiUnicodeGlyphs,
}

impl Field {
    /// All fields in display order. Used both for navigation
    /// (Up/Down indexing) and for rendering.
    pub const ALL: &'static [Field] = &[
        Self::ProviderModel,
        Self::ProviderBaseUrl,
        Self::ProviderApiKey,
        Self::AutoApply,
        Self::UiAutoCopyOnSelect,
        Self::UiUnicodeGlyphs,
    ];

    /// Section header the field belongs under (rendered as a dim
    /// label above the group's first row).
    pub fn section(self) -> &'static str {
        match self {
            Self::ProviderModel | Self::ProviderBaseUrl | Self::ProviderApiKey => "Provider",
            Self::AutoApply => "Approval",
            Self::UiAutoCopyOnSelect | Self::UiUnicodeGlyphs => "UI",
        }
    }

    /// Short label shown left of the value column.
    pub fn label(self) -> &'static str {
        match self {
            Self::ProviderModel => "model",
            Self::ProviderBaseUrl => "base_url",
            Self::ProviderApiKey => "api_key",
            Self::AutoApply => "auto_apply",
            Self::UiAutoCopyOnSelect => "auto_copy_on_select",
            Self::UiUnicodeGlyphs => "unicode_glyphs",
        }
    }

    /// Interaction shape for this field. Drives whether Enter
    /// opens an edit buffer (Text), toggles (Bool), or cycles
    /// (Enum).
    pub fn kind(self) -> FieldKind {
        // API key is Text too; display masking is handled by
        // `sensitive()` / the render layer, not the kind.
        match self {
            Self::ProviderModel | Self::ProviderBaseUrl | Self::ProviderApiKey => {
                FieldKind::Text
            }
            Self::AutoApply => FieldKind::Enum {
                options: &["never", "safe"],
            },
            Self::UiAutoCopyOnSelect | Self::UiUnicodeGlyphs => FieldKind::Bool,
        }
    }

    /// `true` if the field's value should be masked in the
    /// settings display (e.g. API keys). Edit-mode rendering
    /// shows the typed characters so the user can verify
    /// what they're entering.
    pub fn sensitive(self) -> bool {
        matches!(self, Self::ProviderApiKey)
    }

    /// TOML path for `Config::set_in_file`. `(None, key)` =
    /// top-level; `(Some("section"), key)` = nested table.
    pub fn toml_path(self) -> (Option<&'static str>, &'static str) {
        match self {
            Self::ProviderModel => (Some("provider"), "model"),
            Self::ProviderBaseUrl => (Some("provider"), "base_url"),
            Self::ProviderApiKey => (Some("provider"), "api_key"),
            Self::AutoApply => (None, "auto_apply"),
            Self::UiAutoCopyOnSelect => (Some("ui"), "auto_copy_on_select"),
            Self::UiUnicodeGlyphs => (Some("ui"), "unicode_glyphs"),
        }
    }

    /// Read the current value of this field from `cfg`, as a
    /// display string. For `api_key` this is the raw secret -
    /// callers that render to the screen must mask via
    /// [`Self::sensitive`].
    pub fn read(self, cfg: &Config) -> String {
        match self {
            Self::ProviderModel => cfg.provider.model.clone(),
            Self::ProviderBaseUrl => cfg.provider.base_url.clone(),
            Self::ProviderApiKey => cfg.provider.api_key.clone().unwrap_or_default(),
            Self::AutoApply => match cfg.auto_apply {
                AutoApply::Never => "never".to_string(),
                AutoApply::Safe => "safe".to_string(),
            },
            Self::UiAutoCopyOnSelect => cfg.ui.auto_copy_on_select.to_string(),
            Self::UiUnicodeGlyphs => cfg.ui.unicode_glyphs.to_string(),
        }
    }

    /// Apply a user-edited value to `cfg`. Returns `Err(message)`
    /// if the value can't be parsed for this field's type (e.g.
    /// non-bool string for a Bool field, unknown enum variant).
    /// The caller uses the error to push a timeline note and
    /// keep the prior value.
    pub fn apply(self, cfg: &mut Config, value: &str) -> Result<(), String> {
        match self {
            Self::ProviderModel => {
                cfg.provider.model = value.to_string();
                Ok(())
            }
            Self::ProviderBaseUrl => {
                if value.trim().is_empty() {
                    return Err("base_url cannot be empty".into());
                }
                cfg.provider.base_url = value.to_string();
                Ok(())
            }
            Self::ProviderApiKey => {
                // Empty input clears the key (sets to None); non-empty
                // stores as Some(value). On-disk this *always* writes
                // `api_key = ""` (see `to_toml_item`), which figment
                // re-loads as `Some("")` next startup - so the cleared-
                // to-None state is transient until the next reload, at
                // which point it converges to `Some("")`. Both are
                // equivalent for the bearer-auth code path: HttpProvider
                // sends no auth header for None, and `Bearer ` (empty)
                // for `Some("")` - servers that require a real key
                // reject both with 401, servers that don't care accept
                // both. The UI mask shows `<none>` for empty/None and
                // `<redacted>` for any non-empty value.
                cfg.provider.api_key = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
                Ok(())
            }
            Self::AutoApply => match value {
                "never" => {
                    cfg.auto_apply = AutoApply::Never;
                    Ok(())
                }
                "safe" => {
                    cfg.auto_apply = AutoApply::Safe;
                    Ok(())
                }
                other => Err(format!("unknown auto_apply value '{other}' (expected 'never' or 'safe')")),
            },
            Self::UiAutoCopyOnSelect => match value {
                "true" => {
                    cfg.ui.auto_copy_on_select = true;
                    Ok(())
                }
                "false" => {
                    cfg.ui.auto_copy_on_select = false;
                    Ok(())
                }
                other => Err(format!("expected 'true' or 'false', got '{other}'")),
            },
            Self::UiUnicodeGlyphs => match value {
                "true" => {
                    cfg.ui.unicode_glyphs = true;
                    Ok(())
                }
                "false" => {
                    cfg.ui.unicode_glyphs = false;
                    Ok(())
                }
                other => Err(format!("expected 'true' or 'false', got '{other}'")),
            },
        }
    }

    /// Serialize a value string to the [`toml_edit::Item`] used
    /// by `Config::set_in_file`. Text fields write as strings;
    /// Bool fields write as TOML booleans; Enum fields write as
    /// TOML strings (the variant name).
    pub fn to_toml_item(self, value: &str) -> toml_edit::Item {
        match self.kind() {
            FieldKind::Text | FieldKind::Enum { .. } => {
                // For empty optional fields (api_key = "" clears),
                // we still write an empty string. The
                // deserialize path treats empty string as Some("").
                // Users who really want `api_key` removed can edit
                // config.toml directly.
                toml_edit::value(value)
            }
            FieldKind::Bool => {
                // Bool field commits only ever fire with literal
                // "true" / "false" from the toggle handler. If
                // somehow a different string lands here, default
                // to `false` rather than crashing.
                toml_edit::value(value == "true")
            }
        }
    }

    /// Validate `value` and apply it to both `cfg` (live, in
    /// memory) and the config file at `cfg_path`. Single canonical
    /// path for every "settings-style" mutation - both the
    /// `/settings` modal commits and the `/model <name>` slash
    /// command go through this so we have one place that handles
    /// validation, atomic file writes, and the "no config path"
    /// edge case.
    ///
    /// The caller emits the user-visible note text (different
    /// surfaces want different wording: "/settings: model ..." vs
    /// "model switched to ..."). On success, `cfg` reflects the
    /// new value and the file has been updated; on
    /// `ApplyError::Persist`, `cfg` is updated but the file
    /// write failed; on `ApplyError::Validation`, `cfg` is
    /// untouched.
    pub fn apply_and_persist(
        self,
        cfg: &mut Config,
        cfg_path: Option<&Path>,
        value: &str,
    ) -> Result<(), ApplyError> {
        self.apply(cfg, value).map_err(ApplyError::Validation)?;
        let Some(path) = cfg_path else {
            return Err(ApplyError::NoConfigPath);
        };
        let item = self.to_toml_item(value);
        let (section, key) = self.toml_path();
        Config::set_in_file(path, section, key, item)
            .map_err(|e| ApplyError::Persist(e.to_string()))
    }

    /// Next value when the user cycles an enum field. Wraps from
    /// the last option back to the first.
    pub fn cycle_next(self, current: &str) -> Option<&'static str> {
        let FieldKind::Enum { options } = self.kind() else {
            return None;
        };
        let idx = options.iter().position(|&o| o == current).unwrap_or(0);
        let next = (idx + 1) % options.len();
        options.get(next).copied()
    }
}

/// Failure modes for [`Field::apply_and_persist`]. Each variant
/// represents a distinct user-visible outcome the caller may want
/// to message differently. `Validation` means cfg is *unchanged*;
/// `Persist` and `NoConfigPath` mean cfg *was* updated in memory
/// but the file write didn't happen.
#[derive(Debug)]
pub enum ApplyError {
    /// Field-specific validator rejected the value (e.g. empty
    /// base_url, bad enum variant). cfg untouched.
    Validation(String),
    /// `Config::set_in_file` returned an error (parse failure on
    /// the existing file, I/O failure, etc.). cfg updated, file
    /// unchanged.
    Persist(String),
    /// `cfg_path` was `None`. Common when the user runs with
    /// `--config /nonexistent` or in environments where
    /// `project_dirs()` returns nothing.
    NoConfigPath,
}

/// What kind of interaction a field supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Free-form text. Enter on the field opens an edit buffer
    /// pre-seeded with the current value; Enter inside the
    /// buffer commits, Esc cancels.
    Text,
    /// Boolean. Enter on the field toggles the value
    /// immediately; no edit buffer.
    Bool,
    /// One of a fixed set of strings. Enter cycles to the next
    /// option; no edit buffer.
    Enum { options: &'static [&'static str] },
}

/// Edit-mode buffer: a string the user is currently editing for
/// a Text field. The cursor lives at the end of the buffer for
/// v0.1 - left/right cursor movement and intra-buffer editing
/// can be added later if friction surfaces.
#[derive(Debug, Clone)]
pub struct EditBuffer {
    pub buffer: String,
}

/// Active settings-modal state. `None` on `AppState` means the
/// modal is closed.
#[derive(Debug, Clone)]
pub struct SettingsState {
    /// Index into [`Field::ALL`] of the currently highlighted
    /// row. Clamped on every state change.
    pub selected: usize,
    /// `Some` when the user is editing the highlighted field
    /// (Text fields only). `None` = navigation mode.
    pub editing: Option<EditBuffer>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            editing: None,
        }
    }
}

// ----- Key dispatch + commit ------------------------------------ //

/// Dispatch one keystroke while the settings overlay is open.
/// Two sub-modes:
///   * **Navigation** (`editing = None`): Up/Down move selection;
///     Enter activates the highlighted field (edit / toggle /
///     cycle); Esc/Ctrl+C close the overlay.
///   * **Edit** (`editing = Some(_)`): typing / Backspace mutate
///     the buffer; Enter commits the new value to cfg + writes
///     through to config.toml; Esc cancels edit (back to nav).
///
/// Returns `None` when settings isn't open. In nav mode only the
/// listed keys are claimed; everything else is swallowed to keep
/// stray keys from modifying the input pane behind the overlay.
pub(super) fn handle_modal_key(k: KeyEvent, app: &mut AppState) -> Option<Action> {
    app.settings.as_ref()?;
    if is_ctrl_d(k) {
        return Some(Action::Quit);
    }
    let in_edit_mode = app
        .settings
        .as_ref()
        .is_some_and(|s| s.editing.is_some());
    if in_edit_mode {
        handle_edit_key(k, app);
    } else {
        handle_nav_key(k, app);
    }
    Some(Action::Continue)
}

/// Keystrokes while in navigation mode (no field actively being
/// edited).
fn handle_nav_key(k: KeyEvent, app: &mut AppState) {
    match (k.code, k.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.settings = None;
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            if let Some(s) = app.settings.as_mut() {
                s.selected = s.selected.saturating_sub(1);
            }
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            let max = Field::ALL.len().saturating_sub(1);
            if let Some(s) = app.settings.as_mut() {
                s.selected = (s.selected + 1).min(max);
            }
        }
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            activate_field(app);
        }
        _ => {} // swallow other keys
    }
}

/// Keystrokes while editing a Text field's buffer.
fn handle_edit_key(k: KeyEvent, app: &mut AppState) {
    match (k.code, k.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            // Cancel edit - back to nav mode without committing.
            if let Some(s) = app.settings.as_mut() {
                s.editing = None;
            }
        }
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            commit_edit(app);
        }
        (KeyCode::Backspace, _) => {
            if let Some(s) = app.settings.as_mut() {
                if let Some(buf) = s.editing.as_mut() {
                    buf.buffer.pop();
                }
            }
        }
        (KeyCode::Char(c), m) if !m.intersects(KeyModifiers::CONTROL) => {
            if let Some(s) = app.settings.as_mut() {
                if let Some(buf) = s.editing.as_mut() {
                    buf.buffer.push(c);
                }
            }
        }
        _ => {} // swallow other keys (intra-buffer cursor nav is a v0.2 polish)
    }
}

/// Enter behavior on the highlighted field in nav mode:
///   * `ProviderModel` -> open the model picker on top of settings
///     (reuses `/model`'s picker UX; settings stays visible behind
///     it and shows the new value once the picker commits)
///   * Other Text -> open edit buffer pre-seeded with the value
///   * Bool -> toggle and persist immediately
///   * Enum -> cycle to next option and persist immediately
fn activate_field(app: &mut AppState) {
    let Some(state) = app.settings.as_ref() else {
        return;
    };
    let Some(&field) = Field::ALL.get(state.selected) else {
        return;
    };
    // Special-cased Text field: model selection routes through the
    // same picker UI as `/model`, not a raw edit buffer.
    if matches!(field, Field::ProviderModel) {
        super::model_picker::open_picker(app);
        return;
    }
    match field.kind() {
        FieldKind::Text => {
            let current = field.read(&app.cfg);
            if let Some(s) = app.settings.as_mut() {
                s.editing = Some(EditBuffer { buffer: current });
            }
        }
        FieldKind::Bool => {
            let current = field.read(&app.cfg);
            let next = if current == "true" { "false" } else { "true" };
            apply_edit(app, field, next);
        }
        FieldKind::Enum { .. } => {
            let current = field.read(&app.cfg);
            if let Some(next) = field.cycle_next(&current) {
                apply_edit(app, field, next);
            }
        }
    }
}

/// Commit the active edit buffer to the highlighted Text field
/// and exit edit mode.
fn commit_edit(app: &mut AppState) {
    let (field, value) = {
        let Some(state) = app.settings.as_ref() else {
            return;
        };
        let Some(&field) = Field::ALL.get(state.selected) else {
            return;
        };
        let Some(buf) = state.editing.as_ref() else {
            return;
        };
        (field, buf.buffer.clone())
    };
    apply_edit(app, field, &value);
    if let Some(s) = app.settings.as_mut() {
        s.editing = None;
    }
}

/// Apply a /settings commit to `cfg` + persist via
/// [`Field::apply_and_persist`], then surface outcome as a
/// timeline note prefixed with `/settings:`. Note text is the
/// only divergence from `model_picker::switch_model` - both
/// delegate the real work to the same Field method.
fn apply_edit(app: &mut AppState, field: Field, value: &str) {
    let result = field.apply_and_persist(&mut app.cfg, app.cfg_path.as_deref(), value);
    match result {
        Ok(()) => {} // success - silent; UI reflects the new value
        Err(ApplyError::Validation(msg)) => {
            app.timeline
                .push_note(format!("/settings: {} - {msg}", field.label()));
        }
        Err(ApplyError::Persist(msg)) => {
            tracing::warn!(error = %msg, field = field.label(), "settings persist failed");
            app.timeline.push_note(format!(
                "/settings: {} updated in memory only (write failed: {msg})",
                field.label()
            ));
        }
        Err(ApplyError::NoConfigPath) => {
            app.timeline.push_note(format!(
                "/settings: {} updated in memory only (no config file path)",
                field.label()
            ));
        }
    }
}

#[cfg(test)]
#[path = "../tests/tui/settings.rs"]
mod tests;
