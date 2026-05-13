<p align="center">
  <h1 align="center">Lumen</h1>
  <p align="center">A token-efficient, local-LLM-first coding agent CLI. Claude Code "light" with Pi-style modularity.</p>
</p>

<p align="center">
  <img src="assets/lumen_preview.webp" alt="Lumen TUI preview" width="900"/>
</p>

Rust workspace; talks to llama.cpp's `llama-server` out of the box
and to any OpenAI-compatible endpoint (llama-swap, ollama, vLLM,
OpenAI, Anthropic-via-proxy, ...) through the same path.

Built primarily for personal use; published in case it's useful to others.

> **Status: v0.1.0 released.** Install from source (see [Installation](#installation)). Package distribution to AUR / crates.io is intentionally deferred - see [`docs/DESIGN.md`](docs/DESIGN.md#locked-decisions).

## Why

- **Local-first**: built around llama.cpp; no cloud account required to use.
- **Token-efficient**: stable prompt-cache prefix, symbolic retrieval (ripgrep + tree-sitter + LSP) instead of vector RAG by default, token-aware working-set budget (Phase 2).
- **Provider-agnostic**: `Provider` trait abstracts the transport so swapping llama.cpp → Anthropic → OpenAI is a config change, not a refactor.
- **Strict `core` / `cli` boundary**: agent + tools + session live in `lumen-core` with zero UI deps; `lumen-cli` is the ratatui front-end. The compiler enforces the separation.

## Prerequisites

Three things, installed once. Arch users going through the bundled [`packaging/PKGBUILD`](packaging/PKGBUILD) get the first two from pacman automatically; every other path (cargo, Windows, macOS, non-Arch Linux) installs them by hand.

**1. Rust toolchain** - install [rustup](https://rustup.rs/) (it detects your platform and prints the right one-liner; on Windows it's an MSI). The repo pins stable via [`rust-toolchain.toml`](rust-toolchain.toml), so your first `cargo build` auto-fetches the right components.

**2. ripgrep** (`rg` on `$PATH`) - the Grep tool shells out to it. Install instructions per platform are on the [ripgrep README](https://github.com/BurntSushi/ripgrep#installation). Verify with `rg --version`.

**3. An OpenAI-compatible LLM endpoint** - lumen is just the client; it doesn't ship a model or a server. The default target is [`llama-server`](https://github.com/ggml-org/llama.cpp/tree/master/tools/server) from llama.cpp on `http://localhost:8080`; build / install steps are in the [llama.cpp README](https://github.com/ggml-org/llama.cpp#building-the-project), and [prebuilt releases](https://github.com/ggml-org/llama.cpp/releases) are available for Windows / macOS / Linux. Once it's running, point it at a GGUF model: `llama-server -m path/to/model.gguf`.

Any other OpenAI-compatible server works just as well (ollama, vLLM, LM Studio's server mode, llama-swap, or a remote API like OpenAI / Anthropic-via-proxy) - set `[provider] base_url` and `api_key` in `config.toml` accordingly.

## Installation

### From source

```sh
git clone https://github.com/ecdevv/lumen.git
cd lumen
cargo build --release
cargo run --release            # launches the TUI against the configured endpoint
```

### Cargo install

```sh
cargo install --path cli
```

Drops the `lumen` binary into your Cargo bin directory (typically
`~/.cargo/bin/`).

### Arch Linux (local)

```sh
git clone https://github.com/ecdevv/lumen.git
cd lumen/packaging
makepkg -si
```

Installs `lumen` as a system package. Tracks the latest tagged
release. Not yet on AUR - see [`packaging/PKGBUILD`](packaging/PKGBUILD).

## Configuration

Lumen reads a layered config: defaults → `config.toml` (created on
first launch with documented defaults) → `LUMEN_*` env vars → CLI
flags. The most common knobs are exposed live via the `/settings`
slash command in the TUI; `/model <name>` switches the active model
and persists to the config file.

State is split across two XDG-style directories: the **config dir**
holds `config.toml`, and the **data dir** holds session transcripts
and input history. Defaults per platform:

|         | Config dir                             | Data dir                               |
| ------- | -------------------------------------- | -------------------------------------- |
| Linux   | `~/.config/lumen/`                     | `~/.local/share/lumen/`                |
| Windows | `%APPDATA%\lumen\config\`              | `%APPDATA%\lumen\data\`                |
| macOS   | `~/Library/Application Support/lumen/` | `~/Library/Application Support/lumen/` |

Both are overridable via `[paths] config_dir` / `data_dir` in
`config.toml`.

Common keys:

```toml
auto_apply = "never"          # "never" | "safe" - see below

[provider]
base_url = "http://localhost:8080"
model    = ""                 # empty = let the server pick its default
api_key  = ""                 # empty = no auth (local llama.cpp default)

[ui]
auto_copy_on_select = true    # OSC 52 clipboard on drag-release
unicode_glyphs      = true    # set false for legacy terminals
```

`auto_apply` controls only file edits:

- `never` (default) - prompt before every edit
- `safe` - auto-apply edits inside CWD; still prompt for edits
  outside CWD

Shell commands always prompt under both modes - there is no
auto-shell tier in v0.1. Per-command allowlisting is the future
shape for trusting specific shells.

## Inside the TUI

- `Enter` submits; `Shift+Enter` / `Alt+Enter` insert newline
- `/` on empty input opens the slash palette
  - `/help`, `/clear`, `/quit`, `/model`, `/settings`
- `Esc` / `Ctrl+C` cancel turn → clear input → quit (each press has context-aware semantics)
- `Ctrl+D` quits from anywhere (always available, even inside modals)
- `Shift+Tab` cycles approval mode (`never` ↔ `safe`)

## Docs

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - how the code is wired
- [`docs/ROADMAP.md`](docs/ROADMAP.md) - phases, current progress, future considerations
- [`docs/DESIGN.md`](docs/DESIGN.md) - locked decisions, tech stack, cross-cutting rationale
- [`CLAUDE.md`](CLAUDE.md) - Claude Code guidance (project conventions)

## License

Dual-licensed under MIT or Apache-2.0 at your option.
