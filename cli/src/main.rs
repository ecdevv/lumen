//! `lumen-cli` entrypoint.
//!
//! Parses CLI flags + subcommands with `clap`, layers the result over
//! the file/env config in `lumen-core`, initializes tracing, and
//! dispatches to a subcommand handler. The default invocation
//! launches the TUI; the `sessions` subcommand provides `ls`, `rm`,
//! and a `resume` stub awaiting future TUI integration.

mod tui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lumen_core::{AutoApply, Config, logging};

/// Top-level CLI parser. All flags are `global = true` so they can
/// appear before or after a subcommand.
//
// Manual `Debug` impl (below) instead of `#[derive(Debug)]` so that
// `api_key` never lands in `dbg!` / `tracing::*(?cli)` output. Same
// reason `ProviderConfig` redacts its key field.
#[derive(Parser)]
#[command(
    name = "lumen",
    version,
    about = "Local-LLM-first coding agent",
    long_about = "lumen is a token-efficient coding agent CLI focused on \
                  local LLMs first (llama.cpp via OpenAI-compatible HTTP) \
                  with a strict core/cli boundary and packaging-ready layout."
)]
struct Cli {
    /// Path to config file. Default: `<config_dir>/config.toml`.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Override `provider.model`.
    #[arg(long, global = true, value_name = "NAME")]
    model: Option<String>,

    /// Override `provider.base_url`.
    #[arg(long, global = true, value_name = "URL")]
    base_url: Option<String>,

    /// Override `provider.api_key`.
    #[arg(long, global = true, value_name = "KEY")]
    api_key: Option<String>,

    /// Override `auto_apply`: `never` | `safe`.
    //
    // `value_parser` runs our function on the user input and either
    // produces an `AutoApply` or surfaces a clap-formatted error. This
    // keeps the parser policy here in `cli/` rather than leaking a clap
    // dep into `core/`.
    #[arg(long, global = true, value_name = "MODE", value_parser = parse_auto_apply)]
    auto_apply: Option<AutoApply>,

    /// Working directory for tools (defaults to the current dir).
    #[arg(short = 'C', long, global = true, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Mirror logs to stderr in addition to the file. Defaults on in
    /// debug builds; pass to force-on in release.
    #[arg(long, global = true)]
    log_stderr: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Manage stored sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
}

#[derive(Subcommand, Debug)]
enum SessionsAction {
    /// List stored sessions.
    Ls,
    /// Resume a stored session by id. Stub awaiting TUI integration.
    Resume {
        /// Session UUID.
        id: String,
    },
    /// Delete a stored session by id.
    Rm {
        /// Session UUID.
        id: String,
    },
}

impl std::fmt::Debug for Cli {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("config", &self.config)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            // Preserve Some/None shape so the presence vs. absence of
            // a key remains debuggable; just hide the value.
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("auto_apply", &self.auto_apply)
            .field("cwd", &self.cwd)
            .field("log_stderr", &self.log_stderr)
            .field("command", &self.command)
            .finish()
    }
}

fn parse_auto_apply(s: &str) -> std::result::Result<AutoApply, String> {
    match s {
        "never" => Ok(AutoApply::Never),
        "safe" => Ok(AutoApply::Safe),
        other => Err(format!("expected `never` or `safe`; got `{other}`")),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // `{:#}` renders the full anyhow context chain on one line.
            // Tracing isn't guaranteed to be initialized at this point
            // (init failure is one of the things that lands here), so
            // stderr is the safer reporting channel.
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let cfg = load_config(&cli)?;

    // The TUI takes over the screen via crossterm's alt-screen; mirroring
    // logs to stderr there would corrupt the rendered frame. Force off
    // when launching the TUI regardless of `--log-stderr`.
    let is_tui = cli.command.is_none();
    let log_opts = logging::LogOptions {
        stderr: !is_tui && (cli.log_stderr || cfg!(debug_assertions)),
    };
    let log_dir = cfg.paths.data_dir.join("log");
    let _guard = logging::init(&log_dir, log_opts)
        .with_context(|| format!("init logging at {}", log_dir.display()))?;

    match cli.command {
        Some(Command::Sessions { action }) => sessions_cmd(action, &cfg).await,
        None => {
            // `--cwd` flag wins over the process working dir.
            let cwd = match cli.cwd.clone() {
                Some(p) => p,
                None => std::env::current_dir().context("get current dir")?,
            };
            let cfg_path = resolve_config_path(&cli);
            tui::run(cfg, cfg_path, cwd).await
        }
    }
}

/// Apply CLI flag overrides to the file+env-loaded config.
//
// Done here (in `cli/`) rather than in `core::config` so the layering
// remains: defaults -> file -> env -> flags, with flags as the final word
// and `core/` blissfully unaware of clap.
//
// Thin wrapper around [`load_config_from`] that supplies the real
// XDG-derived default config path. The split exists so tests can inject
// a temp-file path without touching `$HOME` / `$XDG_CONFIG_HOME` (which
// would race other parallel tests).
fn load_config(cli: &Cli) -> Result<Config> {
    load_config_from(cli, Config::default_config_path())
}

/// Resolve the active config file (`--config` overrides the supplied
/// `default_path`), load it, and layer CLI flag overrides on top.
//
// `default_path` is `Option` because XDG resolution can fail (e.g. on
// containers without `HOME`); a `None` here means "no file layer at all,"
// matching `Config::load_from(None)`. The pre-fix bug was passing
// `cli.config` straight through, which made the default path *always*
// `None` and silently dropped the file layer for users not passing
// `--config`.
/// Resolve the config path the same way `load_config_from` does,
/// without actually loading. Used by the TUI to know where to
/// write `/model` updates back to.
pub(crate) fn resolve_config_path(cli: &Cli) -> Option<PathBuf> {
    cli.config.clone().or_else(Config::default_config_path)
}

fn load_config_from(cli: &Cli, default_path: Option<PathBuf>) -> Result<Config> {
    let path = cli.config.clone().or(default_path);
    let mut cfg = Config::load_from(path.as_deref())
        .with_context(|| "load config")?;

    if let Some(m) = cli.model.as_deref() {
        cfg.provider.model = m.to_string();
    }
    if let Some(u) = cli.base_url.as_deref() {
        cfg.provider.base_url = u.to_string();
    }
    if let Some(k) = cli.api_key.as_deref() {
        cfg.provider.api_key = k.to_string();
    }
    if let Some(a) = cli.auto_apply {
        cfg.auto_apply = a;
    }

    Ok(cfg)
}

async fn sessions_cmd(action: SessionsAction, cfg: &Config) -> Result<()> {
    let dir = cfg.paths.data_dir.join("sessions");
    match action {
        SessionsAction::Ls => sessions_ls(&dir).await,
        SessionsAction::Rm { id } => sessions_rm(&dir, &id).await,
        SessionsAction::Resume { id: _ } => {
            println!(
                "`sessions resume` is a stub awaiting TUI integration."
            );
            Ok(())
        }
    }
}

async fn sessions_ls(dir: &Path) -> Result<()> {
    if !dir.exists() {
        println!("(no sessions)");
        return Ok(());
    }

    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("read {}", dir.display()))?;

    let mut found = false;
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("scan {}", dir.display()))?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let size = entry.metadata().await.map_or(0, |m| m.len());
        println!("{stem}\t{size:>9} bytes");
        found = true;
    }
    if !found {
        println!("(no sessions)");
    }
    Ok(())
}

async fn sessions_rm(dir: &Path, id: &str) -> Result<()> {
    // Reject anything outside the UUID alphabet (hex + `-`) before
    // joining into the path. Belt-and-braces: under v0.1's threat
    // model the user is trusted ("they can `rm` themselves"), but
    // `dir.join(format!("{id}.jsonl"))` with `id = "../../etc/foo"`
    // would happily compute a path outside `dir`. Format-checking
    // the id closes that gap without pulling in the `uuid` crate.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        bail!("invalid session id `{id}` (expected a UUID; hex + `-` only)");
    }
    let path = dir.join(format!("{id}.jsonl"));
    if !path.exists() {
        bail!("no session with id `{id}` (looked for {})", path.display());
    }
    tokio::fs::remove_file(&path)
        .await
        .with_context(|| format!("remove {}", path.display()))?;
    println!("removed session {id}");
    Ok(())
}

#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
