//! Test helpers shared across TUI submodules.
//!
//! `#[cfg(test)]`-gated; nothing here ships in release. Tests in
//! `app.rs`, `input.rs`, and `render/` all need a fully-wired
//! `AppState` for assertions, and the boilerplate diverges easily.
//! Centralizing it here keeps the three modules' test setups in sync.

use std::path::PathBuf;
use std::sync::Arc;

use lumen_core::provider::HttpProvider;
use lumen_core::{
    Agent, AgentOptions, Config, Session, ToolContext, ToolRegistry,
};
use tokio::sync::Mutex;
use tokio::sync::mpsc::unbounded_channel;

use super::app::AppState;

/// Construct a fully-wired `AppState` against a provider that points at
/// an unreachable port. Tests that need to drive the TUI without
/// actually contacting a model use this and never call `agent.turn`.
pub fn test_app() -> AppState {
    let (tx, _rx) = unbounded_channel();
    let provider = Arc::new(HttpProvider::new("http://127.0.0.1:1", None).unwrap());
    let session = Session::ephemeral();
    let agent = Arc::new(Mutex::new(Agent::new(
        provider,
        ToolRegistry::new(),
        session,
        ToolContext::new(PathBuf::from(".")),
        AgentOptions::default(),
    )));
    AppState::new(Config::default(), tx, agent)
}
