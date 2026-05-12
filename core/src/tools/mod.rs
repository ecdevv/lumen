//! Tools the model can invoke.
//!
//! Every tool implements the [`Tool`] trait: a name, a description, a
//! JSON-Schema for its arguments, and an `async` `invoke`. Tools are kept
//! in a [`ToolRegistry`] keyed by name; the agent loop asks the registry
//! for [`ToolSchema`]s to send to the provider, and dispatches by name
//! when a tool call comes back.
//!
//! Built-ins: `read`, `write`, `edit`, `grep`, `shell`. All are
//! path-sandboxed to [`ToolContext::cwd`] via [`crate::fs::sandboxed`].
//!
//! # Stability and prompt caching
//! Schemas are emitted in stable alphabetical order (the registry uses a
//! `BTreeMap`). The system prompt + tool definitions form the prefix that
//! providers cache, so any non-determinism here would cost cache hits.

pub mod edit;
pub mod grep;
pub mod read;
pub mod shell;
pub mod write;

pub use edit::EditTool;
pub use grep::GrepTool;
pub use read::ReadTool;
pub use shell::ShellTool;
pub use write::WriteTool;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use async_trait::async_trait;

use crate::approval::{ApprovalGate, AutoAcceptGate};
use crate::config::AutoApply;
use crate::error::{Error, Result};
use crate::provider::ToolSchema;

/// Per-invocation context passed to every tool.
///
/// Cheap to clone (one `PathBuf`, two `Arc`s). The agent loop builds
/// it once per turn and shares it across parallel tool calls.
//
// `auto_apply` is an `Arc<AtomicU8>` rather than a plain enum so the
// UI's Shift+Tab toggle can flip the policy at runtime and have the
// change visible to every in-flight and future tool dispatch + to the
// approval gate. The `Arc` is shared with [`AppState`] and (in the
// production wiring) [`super::super::super::cli::TuiApprovalGate`].
//
// Atomic over `RwLock<AutoApply>` because reads dominate writes
// (every tool invocation reads; only Shift+Tab writes), and the
// single-byte payload fits trivially through [`AutoApply::as_u8`].
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Working directory. All file-touching tools sandbox to this root.
    pub cwd: PathBuf,
    /// Shared, atomically-toggleable approval policy. Read via
    /// [`Self::auto_apply`]; mutate via [`Self::set_auto_apply`].
    /// Exposed as a field so the production wiring can share the same
    /// `Arc` with the UI's toggle handler and the approval gate -
    /// `ctx.auto_apply.clone()` everywhere wires them together.
    pub auto_apply: Arc<AtomicU8>,
    /// Approval gate consulted by side-effecting tools (Write/Edit
    /// for proposed diffs; Shell for confirmation prompts).
    /// Defaults to [`AutoAcceptGate`] in tests; production wires
    /// [`super::super::super::cli::TuiApprovalGate`].
    pub gate: Arc<dyn ApprovalGate>,
}

impl ToolContext {
    /// Build a context for the given working directory with default
    /// policy ([`AutoApply::Never`]) and an [`AutoAcceptGate`].
    #[must_use]
    pub fn new(cwd: PathBuf) -> Self {
        Self::with_policy(cwd, AutoApply::default(), Arc::new(AutoAcceptGate))
    }

    /// Build a context with an explicit initial policy and gate.
    /// Use this from production wiring; the atomic is fresh per
    /// context.
    #[must_use]
    pub fn with_policy(cwd: PathBuf, mode: AutoApply, gate: Arc<dyn ApprovalGate>) -> Self {
        Self {
            cwd,
            auto_apply: Arc::new(AtomicU8::new(mode.as_u8())),
            gate,
        }
    }

    /// Snapshot the current approval policy. Atomic load with
    /// `Relaxed` ordering - we don't need stronger semantics because
    /// readers never coordinate with writers beyond "see a value
    /// that was written at some point."
    #[must_use]
    pub fn auto_apply(&self) -> AutoApply {
        AutoApply::from_u8(self.auto_apply.load(Ordering::Relaxed))
    }

    /// Update the approval policy. Subsequent reads via
    /// [`Self::auto_apply`] from any thread holding a clone of this
    /// `Arc` observe the new value.
    pub fn set_auto_apply(&self, mode: AutoApply) {
        self.auto_apply.store(mode.as_u8(), Ordering::Relaxed);
    }
}

/// One model-invocable capability.
///
/// Implementors are typically zero-sized structs; state belongs on the
/// [`ToolContext`] or the registry.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Wire name. Must match what the registry stores under and what the
    /// model emits in its tool calls.
    fn name(&self) -> &'static str;

    /// One- or two-sentence description shown to the model.
    fn description(&self) -> &'static str;

    /// JSON-Schema for the arguments object. Returned as a JSON value so
    /// providers can ferry it to the model unchanged.
    fn parameters(&self) -> serde_json::Value;

    /// Run the tool. `args_json` is the raw JSON the model produced
    /// (unparsed, since each tool deserializes into its own typed struct).
    /// Returns the text to feed back as a tool-result message.
    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String>;

    /// Bundle name + description + parameters into the wire schema.
    //
    // Default impl on the trait: every tool gets the same bundling for
    // free, and concrete tools only fill in the three pieces that vary.
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters(),
        }
    }
}

/// Name -> tool lookup, owned by the agent.
///
/// `BTreeMap` (not `HashMap`) for deterministic iteration order: the
/// schema list goes into the prompt-cached prefix, and reordering it
/// would silently invalidate caches across turns.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Empty registry. Callers register tools manually.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry pre-populated with the standard built-in tools.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(EditTool));
        r.register(Arc::new(GrepTool));
        r.register(Arc::new(ReadTool));
        r.register(Arc::new(ShellTool));
        r.register(Arc::new(WriteTool));
        r
    }

    /// Insert (or replace) a tool by name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name(), tool);
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Schemas in stable alphabetical order - feed straight to the provider.
    #[must_use]
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }

    /// Dispatch a tool call. Unknown names produce an `Error::Tool` so
    /// the agent loop can format a tool-result message containing the
    /// failure (rather than aborting the turn).
    pub async fn invoke(
        &self,
        ctx: &ToolContext,
        name: &str,
        args_json: &str,
    ) -> Result<String> {
        match self.get(name) {
            Some(t) => t.invoke(ctx, args_json).await,
            None => Err(Error::Tool(format!("unknown tool: {name}"))),
        }
    }
}

#[cfg(test)]
#[path = "../tests/tools/mod.rs"]
mod tests;
