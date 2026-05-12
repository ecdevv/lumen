---
name: add-modal
description: Use when the user asks to add a new modal overlay or picker UI to lumen (e.g., "add a settings-style modal for X", "add a picker for Y", "add an overlay that shows Z"). Walks through state declaration, key dispatch, rendering, and the universal-quit reminder that has no compile-time check.
---

# Adding a new modal

Four sites - plus one rule with no compile-time safety net (Ctrl+D).

## 1. Define state + module

File: `cli/src/tui/your_modal.rs` (new)

```rust
//! Module-level doc: what this modal does, when it opens,
//! how its state transitions work.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{Action, AppState};
use super::input::is_ctrl_d;

pub struct YourModalState {
    // selected index, edit buffer, async status, etc.
}

impl YourModalState {
    pub fn new() -> Self { /* ... */ }
}

/// Dispatch one keystroke while the modal is open. Returns
/// `Some(action)` when the key is claimed (nav, commit, dismiss,
/// Ctrl+D); `None` to fall through to the normal pathway.
pub(super) fn handle_modal_key(k: KeyEvent, app: &mut AppState) -> Option<Action> {
    app.your_modal.as_ref()?;

    // **RULE**: Ctrl+D always wins. No exceptions.
    if is_ctrl_d(k) {
        return Some(Action::Quit);
    }

    match (k.code, k.modifiers) {
        // Dismiss: at minimum Esc + Ctrl+C
        (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            app.your_modal = None;
            Some(Action::Continue)
        }
        // ... your nav/commit keys ...
        _ => None, // or Some(Action::Continue) if you want to swallow others
    }
}
```

Register the module: add `mod your_modal;` to `cli/src/tui/mod.rs`.

## 2. Add state field to AppState

File: `cli/src/tui/app.rs`

```rust
pub struct AppState {
    // ... existing fields ...
    pub your_modal: Option<super::your_modal::YourModalState>,
}
```

Initialize in `AppState::new`:

```rust
your_modal: None,
```

## 3. Add render module

File: `cli/src/tui/render/your_modal.rs` (new)

For a **centered overlay** (like help / settings), follow
`help_modal.rs` or `settings_modal.rs`: build a `Vec<Line>`,
size the rect from content, paint `Clear` + `Block` + `Paragraph`.

For a **floating dropdown** (like slash / model picker), call
`super::floating_palette::render_floating_palette` - it owns the
geometry and chrome; you supply title + items + selected index.

Register: add `mod your_modal;` to `cli/src/tui/render/mod.rs`.

## 4. Wire dispatch

### Key dispatch in input.rs

File: `cli/src/tui/input.rs` → `handle_key`

Insert your modal's intercept block at the right precedence. Modals
that can co-exist (e.g., picker opening on top of settings) go in
"innermost first" order so the picker claims keys when both are up.

```rust
if let Some(action) = super::your_modal::handle_modal_key(k, app) {
    return action;
}
```

### Render dispatch in render/mod.rs

File: `cli/src/tui/render/mod.rs` → `render` function

```rust
your_modal::render_modal(frame, app);
```

Render order matters: later draws cover earlier ones. Centered
overlays usually render after floating palettes (so the overlay
appears on top); the help modal renders last (always on top).

## 5. (If needed) Add a UiMsg variant for async work

If your modal does async work (HTTP fetch, file I/O off the event
loop), add a variant to `UiMsg` in `app.rs` and handle it in
`AppState::apply_ui_msg`. Pattern: spawn a task that sends the
result via the channel; the main loop drains and updates state.
See `UiMsg::ModelsLoaded` for an example.

## 6. Trigger

How does the user open the modal? Common patterns:
- A slash command (`/your_modal`): follow `add-slash-command` skill
- A keystroke at empty input (like `/`): add a case in
  `input::handle_key`'s main match
- Programmatic (timer, event from agent): set the state field from
  the relevant handler

## 7. Add tests

File: `cli/src/tests/tui/your_modal.rs` (new) for unit tests of
state behavior, and add input-flow tests in
`cli/src/tests/tui/input.rs` for the keystroke path. Reference
`#[cfg(test)] #[path = "../tests/tui/your_modal.rs"] mod tests;`
at the bottom of `cli/src/tui/your_modal.rs`.

Tests to cover:
- Esc closes
- Ctrl+C closes
- **Ctrl+D quits through** (this is the rule with no compile-time check)
- Key navigation
- Commit behavior
- Dismiss behavior

## Verify

```
cargo build && cargo clippy --all-targets -- -D warnings && cargo test
```

## Reminder

The **only** rule without a compile-time check is "Ctrl+D bypasses
the modal." If you forget to call `is_ctrl_d(k)` at the top of your
dispatch, users can get trapped behind your modal with no way out
except killing the process. The Ctrl+D test catches this if you
write it; please do.
