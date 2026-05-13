# Roadmap

Where we're going, where we are, and what's parked. Updated as
features ship.

## Vision

Lumen is a token-efficient, local-LLM-first coding agent CLI -
Claude Code "light" with Pi-style modularity. Heavy bias toward
local models (llama.cpp first); designed so remote providers
(Anthropic, OpenAI, Gemini) slot in without refactoring.

Seven high-level goals drive the phase plan below:

1. **Core interface** - CLI foundation, file targeting, tooling, diff support, model selection, eventually different providers.
2. **Code review + patching/refactoring** - analyze code, suggest improvements, optionally apply changes.
3. **Validation layer** - run linters, run tests, reject bad outputs (reject-and-retry loop).
4. **Context optimization** *(highest leverage)* - automatic file relevance, smarter eviction, summarization, selective injection, RepoMap, token-budget awareness.
5. **Controlled multi-step workflows** - review → patch → test → retry; light "agent" behavior with deterministic stage boundaries.
6. **Task routing** - if multiple distinct tasks need different system prompts, classify and route.
7. **Full agent orchestration** - sub-agents with isolated contexts, parallel tool execution, inter-agent messaging.

## Phase summary

| Phase | Version | Goal | Status |
|---|---|---|---|
| 0 | v0.1 | Foundation - minimal daily-use loop | shipped as `v0.1.0` (tagged + pushed); distribution (AUR / crates.io) deferred |
| 1 | v0.2 | Validation layer | not started |
| 2 | v0.3 | Context optimization | not started |
| 3 | v0.4 | Code review + patching | not started |
| 4 | v0.5 | Multi-step workflows | not started |
| 5 | v0.6 | Task routing | not started |
| 6 | - | Provider expansion (Anthropic / OpenAI / Gemini / native llama.cpp FFI) | not started |
| 7 | v1.0+ | Agent orchestration | not started |

## Current focus: Phase 0 / v0.1

**Goal: minimal interactive loop you can use daily.**

If you stop mid-step, drop a few resume-bullets right under this
line (next task, blocker, relevant file / commit) so the next
session can pick up without re-deriving context.

| # | Step | Status |
|---|---|---|
| 1 | Cargo workspace + crate skeletons | ✓ |
| 2 | Layered config (figment: defaults → file → env → flag) | ✓ |
| 3 | `tracing` setup (rotating file + stderr in debug) | ✓ |
| 4a | `Provider` trait + types | ✓ |
| 4b | Provider HTTP impl (SSE streaming) | ✓ |
| 5 | `Tool` trait + 5 tools (Read/Write/Edit/Grep/Shell) | ✓ |
| 6 | `Session`: in-memory `Vec<Message>` + JSONL transcript | ✓ |
| 7 | Agent loop (Plan-and-Execute scaffold) | ✓ |
| 8 | CLI entrypoint (clap) | ✓ |
| 9 | TUI (ratatui): input pane, scrollable output, status, streaming | ✓ |
| 10 | Diff preview + apply/reject prompt for Write/Edit | ✓ |
| 11 | Slash commands (`/help`, `/quit`, `/clear`, `/model`, `/settings`) | ✓ |
| 12 | `insta` snapshot tests for diff render + tool I/O | ✓ |
| 13 | AUR `PKGBUILD` skeleton | ✓ |

## Detailed phases

### Phase 1 - Validation Layer / v0.2

- `RunLinter` tool (auto-detect: `cargo clippy`, `ruff`, `eslint`, `golangci-lint`, ...)
- `RunTests` tool with framework auto-detect
- Reject-and-retry loop: agent re-attempts when validator fails, capped retries

### Phase 2 - Context Optimization / v0.3 *(highest leverage)*

- **RepoMap**: tree-sitter symbol extraction, ranked by file size + recency + reference count
- Token-aware working-set budget using `tokenizers` (count *before* sending)
- Old-turn summarization (compress turns > N back into a single system note)
- Auto-load `AGENTS.md`, fallback `CLAUDE.md`
- Selective context injection: only attach files referenced by the current tool call
- Smarter eviction policy keyed on RepoMap relevance

### Phase 3 - Code Review + Patching / v0.4

- `/review` slash command (file or selection → structured findings)
- `/patch` slash command with diff preview
- Symbol-aware refactors (rename, extract) via tree-sitter

### Phase 4 - Multi-Step Workflows / v0.5

- Plan-and-Execute upgraded: real plan node, replan on failure
- Reflection step between plan and execute
- Selective ReAct inside subtasks
- Plan mode (read-only - write tools gated, agent emits structured plan)

### Phase 5 - Task Routing / v0.6

- Rule-based classifier first (cheap heuristics on prompt + open files)
- Per-task system-prompt slots
- Escalate to LLM-as-router only when rules tie

### Phase 6 - Provider Expansion

- Anthropic (with prompt caching headers)
- OpenAI (with prompt caching)
- Gemini
- Native llama.cpp via `llama-cpp-2` FFI (opt-in)

### Phase 7 - Agent Orchestration / v1.0+

- Sub-agents with isolated contexts
- Parallel tool execution
- Inter-agent message passing

## Future Considerations (acknowledged, not designed yet)

These will surface during execution. Flagged here so they're not
surprises and so feature decisions don't accidentally close doors
on them:

- **Hooks / lifecycle events** (pre-tool, post-tool, on-stop) - extensibility point, like Claude Code's hooks
- **External-tool plugin protocol** (MCP-style: stdio JSON-RPC subprocesses exposing tools) - keeps `core` lean and lets the community add tools
- **Non-function-calling model fallback** - current design assumes the provider's `tools` array is honored. Modern function-calling-tuned models handle it; tiny / older local models silently ignore `tools`. `llama-server` (v0 default) speaks tools natively, so the affected population is small. Two paths if complaints surface: (a) **detect-and-warn** - push a one-time timeline note if N turns pass with zero `tool_calls` events while assistant text looks tool-intent-shaped; (b) **text-mode tool calling** - inject tool defs into the system prompt with a custom format, parse calls out of assistant text, maintain a separate prompt-cache prefix
- **Git integration** - auto-commit per accepted edit, branch-per-task option, dirty-tree warnings, blame-aware context
- **Permissions / approval model** - beyond `auto_apply`: per-tool allow/deny lists, dangerous-command gating (`rm -rf`, `git push --force`), session-scoped approvals, **directory-level trust gate** (some form of opt-in before the agent operates in a new cwd; mechanism open - first-run prompt, explicit `--trust .` flag, persisted allowlist, signed marker file, etc.; covers the "agent runs in an unintended directory" case the per-action gates don't reach)
- **Path sandbox: symlink TOCTOU** - `core::fs::sandboxed` resolves `..` / `.` lexically, but a symlink inside `cwd` pointing outside passes the prefix check and the OS follows it on access. Documented in the function's doc comment. Real fix: `Path::canonicalize` after lexical normalization, then re-check the prefix. Acceptable for trusted local use; harden before any "remote agent" or "untrusted prompt" deployment
- **IDE integrations** - `lumen lsp` mode (run as an LSP-compatible server), Neovim plugin, VS Code extension stub
- **Self-update / version notifier** - check latest release on startup (opt-in, off by default)
- **Telemetry** - opt-in only, off by default; if added, anonymized + documented + a single env var to fully disable
- **Themes / color profiles** - true-color, 256-color, no-color; respect `NO_COLOR`
- **Resource limits** - bash timeout, max bytes per file read, max files per grep, max tool calls per turn
- **Multi-root workspaces** - when CWD has nested git repos or `.code-workspace`-like configs
- **Ignore files** - respect `.gitignore`, `.ignore`; introduce `.lumenignore` for agent-specific exclusions
- **Headless / scripting mode** - `lumen run "prompt" --no-tui` for CI/scripting; structured JSON output mode
- **Crash reporting** - local panic dumps written to log dir; explicit "share this file" instruction, never auto-upload
- **Internationalization** - defer; English only for v0.x
- **Unified modal abstraction** - currently 4 modal-ish UIs (help, slash, model picker, settings) share enough shape to refactor under a `Modal` trait or enum. Punted at 2 instances ("guesswork"); revisit at 5+ when one more concrete archetype lands

Each becomes its own design doc when its turn comes. None block v0.1.

## v0.1 verification checklist

Two distinct gates. **Code-complete** (items 1-5) means the
codebase is daily-driver usable when installed from source.
**Distribution** (items 6-7) ships the binary to package
registries and is explicitly deferred - per
[DESIGN.md](DESIGN.md#locked-decisions), v0.1 is "packaging-
ready, not yet uploaded." The repo can be public and the
binary can be daily-driven without items 6-7.

### Code-complete (automated)

1. `cargo build --release` succeeds with zero warnings
2. `cargo clippy --all-targets -- -D warnings` clean
3. `cargo test` passes (including `insta` snapshot tests)

### Code-complete (manual smoke test)

4. `cargo install --path cli` installs `lumen` to `~/.cargo/bin/`
5. With a running `llama-server` on `localhost:8080`:
   - `lumen` launches the TUI
   - Streaming responses render token-by-token
   - "read this file" → ReadFile tool fires, output displayed
   - "edit this file to add X" → diff preview shown, accept/reject works
   - Path sandbox rejects writes outside CWD when `auto_apply=safe`

### Distribution (deferred to "ship-ready")

6. AUR: `makepkg -si` produces and installs an Arch package
   from `packaging/PKGBUILD`; `lumen --version` works
   post-install. `sha256sums=('SKIP')` placeholder needs a
   real hash before submission.
7. crates.io: flip `publish = false` → omitted in both
   `core/Cargo.toml` and `cli/Cargo.toml`, then
   `cargo publish` core first, cli second.

### Future-tied (not v0.1)

- `tests/e2e.rs` spawning the binary against a mock SSE server
  was originally scoped here, but lumen's TUI alt-screen +
  raw-mode initialization makes a subprocess-based happy-path
  test structurally hard without either a PTY harness or a
  headless mode. **Headless / scripting mode**
  (`lumen run "prompt" --no-tui`) is itself deferred to
  [Future Considerations](#future-considerations-acknowledged-not-designed-yet);
  the e2e test lands together with that mode rather than as
  a v0.1 blocker. Existing unit tests already cover the
  underlying happy path at the agent / provider / tool
  layers.
