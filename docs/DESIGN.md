# Design

The "why" behind the project. Locked decisions, tech-stack
choices, cross-cutting rationale, comment-style guide. Stable
across feature work; updated only when we revisit a decision.

For "where we're going" see [`ROADMAP.md`](ROADMAP.md). For "how
the code is wired right now" see [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Language | Rust | Codex's TS→Rust rewrite is the relevant precedent; CC/Gemini/Pi are TS only because their teams already lived there. |
| LLM transport (v0) | HTTP to OpenAI-compatible server | Works with llama.cpp's `llama-server`, llama-swap, ollama, vLLM, OpenAI, Anthropic-via-proxy. Behind a `Provider` trait so native FFI / Anthropic / OpenAI / Gemini slot in later. |
| Retrieval | Symbolic-first (ripgrep + tree-sitter + LSP) | `Retriever` trait left open for vectors later. |
| Boundary | Strict `core/` (lib, no CLI deps) ↔ `cli/` (bin, depends on core) | Enforced by Cargo workspace; `core/Cargo.toml` has no `clap`/`ratatui`/`crossterm` deps. |
| License | Dual MIT/Apache-2.0 | Rust ecosystem standard. |
| Distribution | Packaging-ready, not yet uploaded | `publish = false` blocks accidental crates.io uploads in v0.1; AUR `PKGBUILD` ships locally; submission is an explicit later decision. |
| Platforms | Linux + Windows first-class; macOS best-effort | All code must compile on `x86_64-pc-windows-msvc`; no Linux-specific code without a Windows fallback. AUR for Linux now; Windows installer deferred; brew for macOS deferred. |
| Repo / binary / crates | `lumen` (repo + binary); `lumen-core` (lib), `lumen-cli` (bin) | Short directory names (`core/`, `cli/`); crates.io-ready metadata in place. |
| Config path | `~/.config/lumen/config.toml` | XDG Base Directory spec. |
| Manual ops | AI runs only build/clippy/test commands | Install / publish / packaging are user-run. |

## Tech stack

| Concern | Crate |
|---|---|
| Workspace | Cargo workspace, 2 members |
| CLI args | `clap` (derive) |
| TUI | `ratatui` + `crossterm` + `tui-textarea` |
| Async runtime | `tokio` (multi-thread) |
| HTTP + streaming | `reqwest` + `eventsource-stream` |
| Serialization | `serde` + `serde_json` |
| Config (layered) | `figment` (defaults → file → env → flag) |
| Config write | `toml_edit` (surgical edits, preserves comments) |
| Logging | `tracing` + `tracing-subscriber` (file + stderr sinks) |
| File walk / search | `ignore` + ripgrep (shell-out to `rg --json`; runtime dep on `rg` binary) |
| AST *(Phase 2+)* | `tree-sitter` + per-language grammars (lazy-loaded) |
| Tokenization *(Phase 2+)* | `tokenizers` (HuggingFace) |
| Diffs | `similar` |
| Errors | `thiserror` in core, `anyhow` in cli |
| Snapshot tests | `insta` *(step 12)* |
| Packaging | `cargo` + AUR `PKGBUILD` (`cargo-aur` later) |

## Cross-cutting design decisions

### Prompt caching ("cache reads")

Providers store the *prefix* of your conversation (system prompt +
tool definitions + early messages); reusing that prefix on the next
turn is billed at a fraction (Anthropic ~10%, OpenAI ~50%) and is
faster. llama.cpp does this natively via KV cache.

**Design implication**: the system prompt + tool defs must be a
stable, byte-identical prefix across turns to maximize hit rate.
Baked into `agent.rs` from day one. `/clear` therefore resets the
in-memory messages to the system prompt only (not an empty `Vec`)
via `Session::reset_to_system_prompt`, so the next turn's prefix
still matches the cache.

### ReAct vs Plan-and-Execute

Hybrid. Outer loop = Plan-and-Execute + Reflection; inner subtask
loop = selective ReAct. We accept the "model-as-orchestrator"
determinism ceiling (same as Claude Code) and intercept at
structured stage boundaries.

### Retrieval: symbolic vs RAG

Symbolic-first locked. `Retriever` trait left open so an HNSW-
backed retriever can be added without refactoring callers. Modern
"agentic RAG" / "graph RAG" approaches stay deferred; structured
retrieval (grep/AST/LSP + execution history + summaries + planning)
covers the common cases far more cheaply.

### Memory & history storage

- **Conversation transcripts**: append-only JSONL per session at
  `~/.local/share/lumen/sessions/<uuid>.jsonl`. Each line is one
  event (user msg, assistant msg, tool call, tool result, system
  event). Replayable, diffable, easy to truncate.
- **Prompt-input history** (TUI up-arrow recall, like a shell):
  plain-text file at `~/.local/share/lumen/input_history`, capped
  (last 5000 entries), de-duplicated.
- **Cross-session memory** *(future)*: user-editable markdown at
  `~/.config/lumen/memory.md`, loaded as a system-prompt suffix;
  modified via `/remember` and `/forget`.
- **Logs**: rotating tracing logs at
  `~/.local/share/lumen/log/lumen.log` (separate from transcripts;
  for debugging, not replay).
- All paths follow XDG Base Directory spec; all are configurable.
- We use `data_dir()` (not `state_dir()`) from the `directories`
  crate: `state_dir()` is Linux-only, so sessions/history/logs
  would have nowhere to land on Windows or macOS. Matches the
  convention shared by fish, nushell, and XDG-aware zsh.

### Auto-accept vs ask-before-edit

`auto_apply` config: `never | safe`. Default `never` (every edit
prompts; every shell prompts). `safe` auto-applies file edits
inside CWD and still prompts for edits outside CWD. **Shell
commands always prompt under both modes** - there is intentionally
no "auto-everything" tier. Per-command shell allowlisting
(`/allow <pattern>`) is the right shape for trusting specific
shells, and that lands later.

### Plan mode

A runtime flag that gates write tools and forces the agent to
emit a structured plan instead of taking action. Same shape as
Claude Code's plan mode.

### Session management

`lumen sessions ls | resume <id> | rm <id>` subcommands. Stub in
v0.1, real in Phase 1.

### Slash commands & `/model`

The slash palette opens on `/` at an empty input. Commands accept
inline arguments via `/cmd <args>` - the filter matches only on the
first whitespace-delimited token of the query so the active
command stays highlighted while the user types args.

`/model` has two modes:
- `/model <name>` swaps `cfg.provider.model` in memory **and
  persists** through to the config file (via `Config::set_in_file`,
  using `toml_edit` to surgically rewrite a `[section].key` while
  preserving comments, key order, and other settings).
- Bare `/model` opens a picker submenu that queries
  `Provider::list_models()` (mapped to `GET /v1/models` on
  OpenAI-compatible backends). The model list is cached in
  `AppState::cached_models` so reopening the picker is instant
  within a session.

The picker is the second instance of the floating-palette shape -
the rendering chrome lives in `tui/render/floating_palette.rs` and
is reused by both palette types; state and key dispatch remain
separate per archetype. Two concrete instances was the trigger for
extracting the shared primitive.

### `/settings`

A centered overlay catalog of editable config fields. Each `Field`
enum variant carries metadata to (a) render its row, (b) read its
current value from `Config`, (c) validate + apply a user-edited
value, and (d) serialize to `toml_edit::Item` for surgical
writeback. Field kinds drive the interaction: `Text` opens an
inline edit buffer; `Bool` toggles; `Enum` cycles. Every commit
calls `Config::set_in_file` with the field's (section, key) path -
same persistence machinery as `/model`. Exhaustive matches on
`Field` mean adding a new config field surfaces every site that
must be updated as compile errors.

### First-launch config template

When the resolved config path doesn't exist yet,
`Config::write_template_to` materializes a documented `config.toml`
with all current defaults + per-field comments. Users who prefer
editing the file in their editor get a complete reference on day
one. Subsequent `/settings` and `/model` edits use surgical writes
that preserve any annotations they add. Regeneration: delete the
file; lumen seeds again on next launch.

### Modal architecture

Currently four modal-ish UIs: help overlay (read-only), slash
palette (filterable command list), model picker (filterable model
list), settings overlay (editable field catalog). Each owns its
own state field on `AppState` and its own dispatch module
(`tui/slash.rs`, `tui/model_picker.rs`, `tui/settings.rs`). Shared
rendering primitive (`tui/render/floating_palette.rs`) covers the
two filterable pickers; help + settings each render their own
centered overlay.

Modal unification under a single `Modal` trait/enum was punted at
two instances ("guesswork"). Revisit when a fifth UI lands of the
same archetype.

## Architecture principles

### Core / CLI boundary

`core/` has zero `clap` / `ratatui` / `crossterm` deps - cannot
import them. `cli/` depends on `lumen-core`. The TUI never reaches
into agent internals except through the `Agent` / `Provider` /
`Session` / `ToolRegistry` public surface.

### Test layout

Every `#[cfg(test)]` body lives under a single `tests/` subtree
per crate that mirrors the source layout exactly. A source file at
`core/src/foo/bar.rs` has its test body at
`core/src/tests/foo/bar.rs`, referenced via
`#[cfg(test)] #[path = "..."] mod tests;`. `mod.rs` keeps its name
in the mirror (`tools/mod.rs` ↔ `tests/tools/mod.rs`). `#[path]`
keeps these as unit tests (private items visible), unlike
`cli/tests/` which would force public-API only. `test_support`
modules live under `tests/` too - **no `#[cfg(test)]` code lives
in source files**. Integration tests at `cli/tests/` are still
allowed when actually needed (binary-spawn e2e), but unit tests
universally follow the mirror.

### Snapshot tests (`insta`)

**Inline (`assert_snapshot!(expr, @"...")`) by default. File-based
(`.snap` files under `snapshots/`) when the snapshot is 30+ lines
or would dominate the test fn.**

Two carve-outs that force file-based regardless of size:

1. Content with uniform leading whitespace that's semantic.
   `insta`'s inline-snapshot normalizer strips common leading
   whitespace; the inline literal won't be byte-faithful. Pair with
   a granular `contains(...)` test if you stay inline.
2. Content with tabs or other invisible characters where
   source-code visibility matters more than co-location.

Snapshot files (when used) sit under `<test_dir>/snapshots/` -
insta's default - and ride the same mirror convention as the rest
of the test layout. `.pending-snap` is gitignored; `.snap` is
committed (it's the test oracle).

### Comment style

- `///` doc comments on every public item, describing *what* it
  is and how to use it.
- `//` inline comments only for: (a) design rationale ("why we
  boxed `figment::Error`", "why `Option<&Path>` not
  `Option<PathBuf>`"), (b) non-obvious mechanics unique to this
  code (`#[from]` magic, `bool::then` as `Layer` composition,
  `cfg!` vs `#[cfg]`), or (c) gotchas (`state_dir()` is
  Linux-only, `try_init()` is process-global, `WorkerGuard` must
  be held).
- **Explain each idiom once**, the first time it appears in a
  non-trivial role. Later uses of the same idiom stay uncommented
  - the reader learns by repeated exposure.
- **Cut everything explaining standard syntax** that appears
  hundreds of times: `?`, `match`, `if let`, `Option::map`, basic
  `#[derive(...)]` (Debug/Clone/Default), `String` vs `&str`,
  `PathBuf` vs `&Path`, `Self`. No teaching the language; only
  annotating parts of *this* codebase that aren't obvious from the
  code itself.
- If a comment isn't earning its keep, delete it.
