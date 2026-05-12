# Architecture

Quick map for new contributors. The big picture in 5 minutes; specifics
in the doc-comments on each module.

## Workspace shape

```
lumen/
├── core/         # lumen-core (lib): provider, agent, session, tools,
│                 #                   config, fs, diff, logging, errors.
│                 #                   ZERO TUI dependencies.
└── cli/          # lumen-cli (bin "lumen"): clap entrypoint + ratatui TUI.
                  # Depends on lumen-core.
```

The compiler enforces the boundary: `core/Cargo.toml` has no `clap`,
`ratatui`, or `crossterm` deps, so core *cannot* drift into UI concerns.

## Config flow

```
                                  ┌────────────────┐
                                  │   defaults     │ ← compiled
                                  └────────┬───────┘
                                           │
                                  ┌────────▼───────┐
                                  │  config.toml   │ ← XDG, optional
                                  └────────┬───────┘
                                           │
                                  ┌────────▼───────┐
                                  │   LUMEN_* env  │
                                  └────────┬───────┘
                                           │
                                  ┌────────▼───────┐
                                  │   CLI flags    │
                                  └────────┬───────┘
                                           │
                                           ▼
                                   Config (in-memory)
                                           │
                  ┌────────────────────────┼────────────────────────┐
                  ▼                        ▼                        ▼
            agent / tools             /settings UI            /model UI
            (read-only)               (read + edit)           (read + edit)
                                           │                        │
                                           └──────────┬─────────────┘
                                                      │
                                                      ▼
                                       Config::set_in_file
                                       (surgical toml_edit write,
                                        atomic via tmp + rename)
```

**Single source of truth.** All reads go through `app.cfg`. All writes
funnel through `Config::set_in_file` (preserves comments, ordering,
and other keys). Both `/settings` field commits and `/model <name>`
go through `Field::apply_and_persist` which then calls `set_in_file` -
one canonical path, two surface UIs.

**First-launch seeding.** When the resolved config path doesn't exist
yet, `Config::write_template_to` materializes a documented template
with all current defaults so file-editing users get a complete
reference. Subsequent edits keep using surgical writes; the template
is not regenerated.

## Modal architecture

Three modal-ish UIs today, each with its own state field on
`AppState`:

| Modal | Trigger | State field | Module |
|---|---|---|---|
| Help overlay | `/help` | `show_help: bool` | `tui/render/help_modal.rs` |
| Slash palette | `/` (empty input) | `slash_palette: Option<SlashPalette>` | `tui/slash.rs` |
| Model picker | `/model` (bare) or `/settings → model` | `model_picker: Option<ModelPickerState>` | `tui/model_picker.rs` |
| Settings overlay | `/settings` | `settings: Option<SettingsState>` | `tui/settings.rs` |

**Dispatch order in `input::handle_key`:**

```
approval prompt? ──yes──→ approval dispatch
   │ no
   ▼
help overlay open? ──yes──→ swallow + dismiss on Esc/Ctrl+C
   │ no
   ▼
model picker open? ──claim?──→ picker dispatch (nav/commit/dismiss)
   │ no/fall-through
   ▼
settings overlay open? ──claim?──→ settings dispatch (nav/edit)
   │ no/fall-through
   ▼
slash palette open? ──claim?──→ palette dispatch
   │ no/fall-through
   ▼
main match (slash trigger, chord keys, textarea routing)
   │
   ▼
post-pass: sync_palette + sync_picker
```

**Universal Ctrl+D.** Every modal honors Ctrl+D as quit (so users
can't get trapped). The `is_ctrl_d` helper in `input.rs` is the
single match - each modal calls it at the top of its dispatch.

**Mutual exclusion.** Modals are largely independent state fields;
in practice they're mutually exclusive except for the documented
case of `/settings → model field` opening the model picker on top
of settings (picker claims keys; settings remains visible behind).

## "How do I add X?"

### Add a new config field

1. Add the field to `Config` (or a sub-struct) in `core/src/config.rs`,
   with `#[serde(default)]` semantics. Add to the relevant `Default`
   impl if non-trivial.
2. Update `Config::write_template_to` with a section describing the
   field, so first-launch users see it in their seeded config.toml.
3. (Optional, if user-facing) Add a `Field` variant in
   `cli/src/tui/settings.rs`. The exhaustive matches in `Field`'s
   methods (`section`, `label`, `kind`, `toml_path`, `read`, `apply`,
   `to_toml_item`, `sensitive`) will fail to compile until you handle
   the new variant - that's the safety net.
4. Add the variant to `Field::ALL` so it appears in the UI.

### Add a new slash command

1. Add to `COMMANDS` array in `cli/src/tui/slash.rs`.
2. Add a variant to `SlashAction`.
3. Handle the variant in `slash::execute_action`'s exhaustive match.
4. Add a unit test verifying filter behavior in
   `tests/tui/slash.rs`.

### Add a new modal

1. Define state struct + module in `cli/src/tui/your_modal.rs`,
   include key dispatch + render entry points.
2. Add `Option<YourState>` field to `AppState`.
3. Add module declaration in `cli/src/tui/mod.rs`.
4. Add render call in `cli/src/tui/render/mod.rs::render` (after the
   conversation pane / input / status bar; before help overlay if
   help should remain on top).
5. Wire dispatch in `input::handle_key`. **Don't forget the
   `is_ctrl_d` early return** - we have no compile-time check for
   this yet; rely on the `handle_modal_key` shape established by
   the existing three modals.

## Module map (cli/)

```
cli/src/
├── main.rs                 # clap entrypoint; resolves cfg + cfg_path
├── tui/
│   ├── mod.rs              # tui::run - terminal setup, event loop
│   ├── app.rs              # AppState (all live state) + UiMsg
│   ├── input.rs            # keystroke → action dispatcher, history,
│   │                       #            mouse, chord helpers
│   ├── slash.rs            # /-palette: registry, filter, dispatch
│   ├── model_picker.rs     # model picker state, /model command,
│   │                       #                     switch_model
│   ├── settings.rs         # Field enum, ApplyError, settings overlay
│   │                       #             state + dispatch
│   ├── approval.rs         # TuiApprovalGate (tool-side hook)
│   ├── clipboard.rs        # OSC 52
│   ├── timeline.rs         # message log model
│   ├── markdown.rs         # inline markdown parser
│   └── render/             # ratatui-only widgets, split per region
│       ├── mod.rs                # render() entry
│       ├── layout.rs             # geometry / wrap math
│       ├── conversation_pane.rs  # chat history
│       ├── input_pane.rs         # textarea + token-usage title
│       ├── status_bar.rs         # bottom bar (tokens · hint | model · cwd)
│       ├── approval_panel.rs     # pinned approval region
│       ├── help_modal.rs         # centered help overlay
│       ├── settings_modal.rs     # centered settings overlay
│       ├── slash_palette.rs      # thin caller of floating_palette
│       ├── model_picker.rs       # thin caller of floating_palette
│       └── floating_palette.rs   # shared primitive: dropdown chrome
```

## Path display in the UI

All UI path rendering routes through `cli/src/tui/app.rs::display_path(p, cwd)`:
cwd-relative bare (`test.txt`), then home-relative (`~/...`), then absolute.
Outside-cwd paths intentionally keep their absolute form as a leak signal.

Surfaces that go through it:

- Tool-call args line (`● write(...)`) - via `short_args_with_paths` which
  parses the JSON, substitutes the `path` field, re-serializes
- Streaming spinner label (`Writing test.txt…`) - via `format_tool_action`
- Approval modal header (`Apply edit to test.txt`) - via `approval_header_diff`
- Approval diff body (`--- a/test.txt`, `+++ b/test.txt`) - via
  `shorten_diff_paths` which scans the diff text and rewrites the header lines

The status bar's cwd display deliberately uses `pretty_path` (home-relative,
not cwd-relative): a cwd rendered relative to itself degenerates to `.` and
loses the "which project am I in?" signal.

## Test layout

See [`DESIGN.md#test-layout`](DESIGN.md#test-layout) for the
convention. In short: every `#[cfg(test)]` body lives in a `tests/`
mirror tree per crate, referenced from the source file via
`#[cfg(test)] #[path = "..."] mod tests;`.
