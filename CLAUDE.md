# CLAUDE.md

Guidance for Claude Code (claude.ai/code) when working in this repo.
Loaded every session - kept lean. Deep context lives in `docs/`.

## What lumen is

A token-efficient, local-LLM-first coding agent CLI - Claude Code
"light" with Pi-style modularity. Rust workspace, strict
`core/` (lib) ↔ `cli/` (bin) boundary. llama.cpp-first via an
OpenAI-compatible HTTP `Provider` trait; remote providers
(Anthropic / OpenAI / Gemini) slot in later without refactoring.

## Doc map

Read the one you need; don't preload them all:

- [`README.md`](README.md) - install + build + quick usage (user-facing)
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - how the code is wired *right now*
- [`docs/ROADMAP.md`](docs/ROADMAP.md) - phases, current progress, future considerations
- [`docs/DESIGN.md`](docs/DESIGN.md) - locked decisions, tech stack, cross-cutting rationale

For "how do I add X?" workflows (config field / slash command /
modal), see the project-local skills under `.claude/skills/`.

## Progress

**Phase 0 / v0.1.** **Code-complete** (steps 1-13 + post-step-13
hardening pass closed the v0.1 code-review gaps: `/clear` reseeds
the system prompt; shell always prompts under both `auto_apply`
modes; `list_models` gets a per-request timeout; `sessions rm`
validates its id; stale `Always`-variant comments swept). Next:
**manual smoke test** + **GitHub push** as code-complete v0.1.
Distribution (AUR / crates.io) is explicitly deferred per
[`docs/DESIGN.md`](docs/DESIGN.md#locked-decisions) - the binary
is packaging-ready, not yet uploaded. Full phase + step table in
[`docs/ROADMAP.md`](docs/ROADMAP.md).

If you stop mid-step, drop a few resume-bullets right under this
line (next task, blocker, relevant file / commit) so the next
session can pick up without re-deriving context.

## Working rules

- **Allowed commands**: `cargo build`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `cargo insta accept`, `cargo insta review`,
  `cargo insta test`. Anything else (install, packaging, publishing)
  is user-run; paste output and continue from there.
- **Boundary**: `core/Cargo.toml` has no `clap`/`ratatui`/`crossterm`
  deps - compiler enforces. Don't add UI deps to core.
- **Surgical changes**: touch only what's needed. Don't refactor
  adjacent code. Match existing style.
- **Test layout**: every `#[cfg(test)]` body lives under a `tests/`
  subtree per crate that mirrors source layout. Source at
  `<crate>/src/foo/bar.rs` → tests at `<crate>/src/tests/foo/bar.rs`,
  referenced via `#[cfg(test)] #[path = "..."] mod tests;`. No
  `#[cfg(test)]` code inside source files. Full convention in
  [`docs/DESIGN.md`](docs/DESIGN.md#test-layout).
- **Comment style**: `///` doc-comments on every public item;
  `//` inline only for rationale / non-obvious mechanics / gotchas.
  Each idiom explained once. No teaching basic syntax. Full
  guidance in [`docs/DESIGN.md`](docs/DESIGN.md#comment-style).
- **Platforms**: Linux + Windows first-class. All code must
  compile on `x86_64-pc-windows-msvc`; no Linux-only paths
  without a Windows fallback. macOS best-effort.

## When designing new features

Before building, check:
1. [`docs/ROADMAP.md`](docs/ROADMAP.md) for whether the feature
   touches a future phase - don't accidentally close doors on
   downstream work.
2. [`docs/DESIGN.md`](docs/DESIGN.md) for any cross-cutting
   decision the feature depends on (e.g., prompt-cache prefix
   stability, the `Provider` trait shape, persistence path).
3. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for where in
   the code the feature plugs in.

Then prefer the "how do I add X?" skill if one fits the shape.
