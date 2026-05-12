//! Layered configuration for `lumen-core`.
//!
//! Loading order - later layers override earlier ones:
//! 1. Compile-time defaults
//! 2. Optional TOML file at `<config_dir>/config.toml`
//! 3. Environment variables prefixed with `LUMEN_`
//!
//! Use `__` (double underscore) in env var names to address nested keys,
//! e.g. `LUMEN_PROVIDER__BASE_URL=http://localhost:8080`.
//!
//! CLI-flag overrides are applied later by the `lumen-cli` crate.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Top-level configuration.
//
// `#[serde(default)]` + figment's defaults layer means partial config
// files Just Work: missing fields fall back to `Default::default()`
// instead of erroring out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// LLM provider settings.
    pub provider: ProviderConfig,
    /// Edit-application policy.
    pub auto_apply: AutoApply,
    /// On-disk path locations.
    pub paths: PathsConfig,
    /// TUI surface preferences.
    pub ui: UiConfig,
}

/// LLM provider settings.
//
// Manual `Debug` impl rather than derive so the API key never lands in
// `dbg!` output or tracing logs that capture config via `Debug`. Even
// if no current call site formats this value, the derived impl is a
// trap waiting for the first careless `tracing::debug!(?cfg, ...)`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// HTTP base URL of the OpenAI-compatible endpoint.
    pub base_url: String,
    /// Model identifier sent to the provider. Llama.cpp servers typically ignore this.
    pub model: String,
    /// API key, if the endpoint requires one. `None` for local llama.cpp.
    pub api_key: Option<String>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field(
                "api_key",
                // Preserve Some/None shape so the absence vs presence
                // of a key is still visible; just hide the value.
                &self.api_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Edit-application policy.
//
// `#[serde(rename_all = "lowercase")]` makes the wire form
// `"never" | "safe" | "always"` instead of the variant casing.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AutoApply {
    /// Prompt before every edit, and prompt for every shell command.
    /// The "Claude Code default" - safe out of the box, escalates only
    /// when the user explicitly toggles (Shift+Tab) to "auto edits"
    /// or picks "Accept all this session" inside a prompt.
    //
    // `#[default]` picks this variant for `AutoApply::default()`;
    // `#[derive(Default)]` alone wouldn't know which to choose.
    #[default]
    Never,
    /// Auto-apply file edits; still prompt for every shell command.
    /// Matches Claude Code's "accept edits" mode.
    //
    // There is intentionally no "auto-everything" tier - per-command
    // shell allowlisting (`/allow <pattern>`) is the right shape for
    // letting trusted shells through, and that lands as a later
    // iteration once the slash-command infrastructure is in place.
    // Until then, shell always prompts under both `Never` and `Safe`.
    Safe,
}

impl AutoApply {
    /// Encode as a single byte for storage in an `AtomicU8` (the
    /// shape used to thread a runtime-toggleable policy across the
    /// tool layer, the approval gate, and the UI).
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Never => 0,
            Self::Safe => 1,
        }
    }

    /// Decode from the atomic byte representation. Unknown bytes
    /// resolve to `Never` (fail-safe-closed: a corrupted atomic
    /// shouldn't silently escalate the user's blast radius).
    #[must_use]
    pub const fn from_u8(b: u8) -> Self {
        match b {
            1 => Self::Safe,
            _ => Self::Never,
        }
    }

    /// Toggle between modes. `Never` <-> `Safe`. Plan mode (a future
    /// read-only third tier, Phase 4) will extend this into a
    /// 3-cycle once it lands.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Never => Self::Safe,
            Self::Safe => Self::Never,
        }
    }

    /// Short human-readable label for the policy-hint row.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Never => "ask edits",
            Self::Safe => "auto edits",
        }
    }
}

/// TUI surface preferences. Expand as more UI knobs accrue (theme,
/// color profile, default scroll-step, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// When `true` (default), mouse drag-then-release auto-copies the
    /// selected text to the system clipboard via OSC 52. Set to
    /// `false` to keep the selection visible without touching the
    /// clipboard.
    //
    // There's no manual copy chord. Every candidate
    // (`Ctrl+Shift+C`, `Alt+C`, `Ctrl+Y`, ...) breaks in some
    // terminal/OS combination: kitty and gnome-terminal grab
    // `Ctrl+Shift+C` for their own clipboard action; macOS Option
    // produces special characters instead of an Alt-modifier event;
    // tui-textarea reserves `Ctrl+Y` for paste. Drag-and-release is
    // the universal gesture - it's the same input the user would
    // have made for terminal-native selection had we not been in
    // alt-screen mode, so we just turn it into a copy directly.
    // Users who want the clipboard untouched can flip this off.
    pub auto_copy_on_select: bool,
    /// When `true` (default), the TUI uses unicode box-drawing and
    /// pointer glyphs (`❯`, `⎿`, `●`, `✗`, etc.). Some legacy
    /// terminals or limited-font setups (basic `xterm` without a
    /// fallback font, certain TTY consoles) render these as boxes
    /// or substitute glyphs. Set `false` to fall back to ASCII
    /// equivalents (`>`, `*`, `x`, ...) across the UI.
    pub unicode_glyphs: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            auto_copy_on_select: true,
            unicode_glyphs: true,
        }
    }
}

/// XDG-style on-disk locations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    /// Directory holding `config.toml` and other user-edited files.
    pub config_dir: PathBuf,
    /// Directory holding session transcripts, input history, and logs.
    /// Maps to `XDG_DATA_HOME` (`~/.local/share/lumen/`) on Linux,
    /// `%APPDATA%\lumen\` on Windows, `~/Library/Application Support/lumen/`
    /// on macOS. Following the convention shared by fish, nushell, and
    /// XDG-aware zsh: history-style files belong in data, not state.
    pub data_dir: PathBuf,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8080".to_string(),
            model: "default".to_string(),
            api_key: None,
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        match project_dirs() {
            Some(d) => Self {
                config_dir: d.config_dir().to_path_buf(),
                // `data_dir()` is cross-platform: XDG_DATA_HOME on
                // Linux, `%APPDATA%` on Windows, ~/Library/Application
                // Support on macOS.
                data_dir: d.data_dir().to_path_buf(),
            },
            // Last-resort fallback if XDG/platform dirs can't be resolved
            // (e.g. headless containers without $HOME). Local relative
            // paths so we never silently write to /.
            None => Self {
                config_dir: PathBuf::from(".lumen/config"),
                data_dir: PathBuf::from(".lumen/data"),
            },
        }
    }
}

// `ProjectDirs::from(qualifier, organization, application)` resolves to:
//   Linux   -> ~/.config/<app>
//   Windows -> %APPDATA%\<app>
//   macOS   -> ~/Library/Application Support/<app>
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "lumen")
}

impl Config {
    /// Load defaults, then layer the standard config file (if present),
    /// then layer `LUMEN_*` environment variables.
    pub fn load() -> Result<Self> {
        let path = Self::default_config_path();
        Self::load_from(path.as_deref())
    }

    /// Load with an explicit config-file path. `None` skips the file layer.
    //
    // Takes `Option<&Path>` (borrowed) so callers can pass a string
    // literal, `&PathBuf`, or `&Path` interchangeably without giving
    // up ownership of the path.
    pub fn load_from(config_path: Option<&Path>) -> Result<Self> {
        let mut fig = Figment::new().merge(Serialized::defaults(Self::default()));

        if let Some(path) = config_path {
            if path.exists() {
                fig = fig.merge(Toml::file(path));
            }
        }

        // `.split("__")` makes `LUMEN_PROVIDER__BASE_URL` resolve to
        // the nested key `provider.base_url` rather than a flat one.
        fig = fig.merge(Env::prefixed("LUMEN_").split("__"));

        // `?` here propagates `figment::Error` via our manual `From`
        // impl in `error.rs`, lifting it into `Error::Config`.
        Ok(fig.extract()?)
    }

    /// `<config_dir>/config.toml`, derived from XDG.
    #[must_use]
    pub fn default_config_path() -> Option<PathBuf> {
        project_dirs().map(|d| d.config_dir().join("config.toml"))
    }

    /// Surgically write a key to the TOML file at `path`,
    /// preserving every other key and any comments / whitespace.
    /// `section = Some("provider")` targets `[provider].key`;
    /// `section = None` targets a top-level key.
    ///
    /// The file is created (with parent directories) if it
    /// doesn't exist. Write is atomic via tmp-file + rename so a
    /// crash mid-write leaves the original config intact.
    ///
    /// Backs both the `/model <name>` slash command and the
    /// `/settings` modal's commit paths. We use `toml_edit`
    /// (not `toml`) because user-edited config files often have
    /// comments and non-default key ordering; a full serialize-
    /// roundtrip would flatten that out.
    pub fn set_in_file(
        path: &Path,
        section: Option<&str>,
        key: &str,
        item: toml_edit::Item,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::Error::Io)?;
        }
        let existing = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(crate::error::Error::Io(e)),
        };
        let mut doc: toml_edit::DocumentMut = existing.parse().map_err(
            |e: toml_edit::TomlError| {
                crate::error::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("config.toml parse error: {e}"),
                ))
            },
        )?;
        match section {
            Some(s) => {
                // Ensure the `[section]` table exists, then
                // overwrite the key under it.
                let table_item = doc
                    .entry(s)
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
                if let Some(table) = table_item.as_table_mut() {
                    table[key] = item;
                }
            }
            None => {
                // Top-level key (no surrounding table).
                doc[key] = item;
            }
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, doc.to_string()).map_err(crate::error::Error::Io)?;
        std::fs::rename(&tmp, path).map_err(crate::error::Error::Io)?;
        Ok(())
    }

    /// Thin wrapper over [`Self::set_in_file`] for the common
    /// `/model <name>` path. Kept as a named helper so the call
    /// site reads as intent (`set_model_in_file(p, "gpt-4o")`)
    /// rather than wire-shape mechanics.
    pub fn set_model_in_file(path: &Path, model: &str) -> Result<()> {
        Self::set_in_file(path, Some("provider"), "model", toml_edit::value(model))
    }

    /// Write a documented template config file at `path`, using
    /// `cfg`'s current values as the seed contents. Each field is
    /// preceded by a comment describing what it does, so users
    /// who prefer editing the file in their editor get a complete
    /// reference on first launch.
    ///
    /// **Maintainer note**: when you add a new field to `Config`,
    /// update this template and add a section so the field is
    /// discoverable. There's no automated check - the template is
    /// hand-coded on purpose so the documentation quality stays
    /// curator-grade.
    ///
    /// Called once when the resolved config path doesn't exist
    /// yet (TUI startup). Subsequent edits go through
    /// [`Self::set_in_file`] (surgical) so user-added comments and
    /// reordering survive.
    pub fn write_template_to(path: &Path, cfg: &Config) -> Result<()> {
        let auto_apply_str = match cfg.auto_apply {
            crate::config::AutoApply::Never => "never",
            crate::config::AutoApply::Safe => "safe",
        };
        let template = format!(
            "# Lumen configuration.\n\
             # Edit and save; lumen reads this file on next startup. Delete any line\n\
             # to fall back to the compiled default, or delete the whole file to start\n\
             # fresh - lumen regenerates it on the next launch.\n\
             #\n\
             # Settings can also be edited live via the `/settings` slash command.\n\
             \n\
             # Approval policy.\n\
             #   never = prompt for every edit + every shell command\n\
             #   safe  = auto-apply edits inside CWD; still prompt for shell\n\
             auto_apply = \"{auto_apply}\"\n\
             \n\
             [provider]\n\
             # OpenAI-compatible HTTP endpoint. Default is local llama.cpp's\n\
             # `llama-server`; multi-model proxies (llama-swap, ollama, vLLM) and\n\
             # remote providers (OpenAI, Anthropic-via-proxy) work too.\n\
             base_url = \"{base_url}\"\n\
             \n\
             # Model identifier sent in completion requests. Local single-model\n\
             # servers usually ignore this; multi-model proxies route on it.\n\
             model = \"{model}\"\n\
             \n\
             # API key for authenticated providers. Leave empty for local servers\n\
             # that don't require auth.\n\
             api_key = \"{api_key}\"\n\
             \n\
             [ui]\n\
             # Auto-copy selected text to the system clipboard (OSC 52) when a\n\
             # mouse drag-and-release completes.\n\
             auto_copy_on_select = {auto_copy_on_select}\n\
             \n\
             # Use unicode box-drawing / pointer glyphs in the TUI. Set false for\n\
             # legacy terminals without unicode support.\n\
             unicode_glyphs = {unicode_glyphs}\n",
            auto_apply = auto_apply_str,
            base_url = toml_basic_escape(&cfg.provider.base_url),
            model = toml_basic_escape(&cfg.provider.model),
            api_key = toml_basic_escape(cfg.provider.api_key.as_deref().unwrap_or("")),
            auto_copy_on_select = cfg.ui.auto_copy_on_select,
            unicode_glyphs = cfg.ui.unicode_glyphs,
        );

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(crate::error::Error::Io)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, template).map_err(crate::error::Error::Io)?;
        std::fs::rename(&tmp, path).map_err(crate::error::Error::Io)?;
        Ok(())
    }
}

/// Minimal escape for TOML basic strings: backslash + double quote.
/// Config values we expect (URLs, model names, API keys) don't
/// realistically contain newlines or control chars, but `\` and `"`
/// occur naturally in some keys/passwords - escape them so the
/// generated file always parses.
fn toml_basic_escape(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
