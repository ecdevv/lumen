//! `lumen-core` - the framework layer for lumen.
//!
//! This crate is intentionally CLI-agnostic. It must never depend on
//! `clap`, `ratatui`, `crossterm`, or any other interactive-layer crate.
//! The interactive surface lives in the `lumen-cli` crate.

pub mod agent;
pub mod approval;
pub mod config;
pub mod diff;
pub mod error;
pub mod fs;
pub mod logging;
pub mod provider;
pub mod session;
pub mod tools;

pub use agent::{Agent, AgentEvent, AgentOptions, CORE_SYSTEM_PROMPT};
pub use approval::{AlwaysRejectGate, ApprovalGate, AutoAcceptGate, REJECTION_PREFIX, Verdict};
pub use config::{AutoApply, Config, PathsConfig, ProviderConfig, UiConfig};
pub use error::{Error, Result};
pub use session::{Session, SessionId, TranscriptEvent};
pub use tools::{Tool, ToolContext, ToolRegistry};
// Re-export `toml_edit` so CLI callers can build `toml_edit::Item`s
// for `Config::set_in_file` without taking their own direct
// dependency on the crate.
pub use toml_edit;
