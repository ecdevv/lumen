//! Tracing subscriber initialization for `lumen-core`.
//!
//! Sets up the global tracing dispatcher with:
//!   * a daily-rotated file appender at `log_dir/lumen.log.<date>`,
//!   * an optional stderr layer (disabled in TUI mode to avoid screen
//!     corruption),
//!   * filter directives sourced from `LUMEN_LOG` (preferred), then
//!     `RUST_LOG`, then defaulting to `info`.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::error::{Error, Result};

/// Initialization options for logging.
#[derive(Debug, Clone, Copy)]
pub struct LogOptions {
    /// Mirror logs to stderr in addition to the file. Set `false` when
    /// running the TUI to avoid corrupting the rendered screen. Defaults
    /// to `true` in debug builds and `false` in release.
    pub stderr: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        // `cfg!(...)` is the expression form of `#[cfg(...)]` - a
        // compile-time bool, true for debug builds, false for release.
        Self { stderr: cfg!(debug_assertions) }
    }
}

/// Initialize the global tracing subscriber.
///
/// Returns a [`WorkerGuard`] which must be held by the caller until the
/// program exits - its `Drop` impl flushes buffered log records. Drop
/// the guard early and you lose any in-flight log records.
///
/// # Errors
/// * [`Error::Io`] if `log_dir` cannot be created.
/// * [`Error::TracingInit`] if a global subscriber is already installed.
pub fn init(log_dir: &Path, opts: LogOptions) -> Result<WorkerGuard> {
    std::fs::create_dir_all(log_dir)?;

    let appender = tracing_appender::rolling::daily(log_dir, "lumen.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    // Filter precedence: env-specific override -> standard tracing env ->
    // hard-coded default.
    let filter = EnvFilter::try_from_env("LUMEN_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // ANSI color disabled because terminal escape codes don't belong
    // in a log file.
    let file_layer = fmt::layer().with_writer(non_blocking).with_ansi(false);

    // `bool::then(|| ...)` returns `Some(value)` if true, else `None`.
    // `Option<L>` itself implements `Layer` (a no-op when None), so
    // this composes cleanly without an if/else that branches on
    // subscriber *types* - which would conflict in Rust's type system.
    let stderr_layer = opts
        .stderr
        .then(|| fmt::layer().with_writer(std::io::stderr));

    // `try_init()` installs as the global dispatcher; only one per
    // process is allowed, so a second call returns Err.
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .map_err(|e| Error::TracingInit(e.to_string()))?;

    Ok(guard)
}
