//! Errors used throughout `lumen-core`.

use thiserror::Error;

/// Convenience alias for `Result` with a `lumen-core` [`Error`].
//
// Shadowing `Result` lets every function in the crate write
// `-> Result<T>` instead of `-> Result<T, Error>`. Standard idiom.
pub type Result<T> = std::result::Result<T, Error>;

/// All errors that can be returned from `lumen-core`.
///
/// `figment::Error` is ~200 bytes, so it's boxed to keep `Result<_, Error>`
/// cheap on the success path (clippy: `result_large_err`).
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration loading or parsing failure.
    #[error("config: {0}")]
    Config(Box<figment::Error>),

    /// Underlying I/O failure.
    //
    // `#[from]` (thiserror sugar) auto-generates an
    // `impl From<std::io::Error> for Error`, so `?` on any io::Result
    // propagates cleanly without a manual conversion.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// XDG project directories could not be determined for the current platform.
    #[error("could not determine project directories for the current platform")]
    NoProjectDirs,

    /// Tracing subscriber failed to initialize (already installed, or invalid filter).
    #[error("tracing init: {0}")]
    TracingInit(String),

    /// HTTP transport failure (connection, timeout, response read, etc.).
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON encoding or decoding failure.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Upstream LLM provider returned a non-success HTTP status.
    #[error("provider returned {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated by the caller if large).
        body: String,
    },

    /// Server-Sent-Events stream parse failure.
    #[error("sse: {0}")]
    Sse(String),

    /// Tool-domain failure surfaced back to the model - bad arguments,
    /// sandbox rejection, "old_string not found", non-zero shell exit, etc.
    /// Distinct from `Io` / `Json` because these are *expected* failures
    /// the agent loop renders into a tool-result message rather than
    /// aborting the turn.
    #[error("tool: {0}")]
    Tool(String),
}

// Manual `From` impl instead of `#[from]`: the derived version would
// require the variant's payload to match the source type exactly, but
// our `Config` variant holds `Box<figment::Error>` (boxed for size),
// not bare `figment::Error`. `?` still propagates thanks to this.
impl From<figment::Error> for Error {
    fn from(e: figment::Error) -> Self {
        Self::Config(Box::new(e))
    }
}
