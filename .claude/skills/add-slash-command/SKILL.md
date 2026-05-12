---
name: add-slash-command
description: Use when the user asks to add a new slash command to lumen (e.g., "add a /foo slash command", "add a slash command for X"). Walks through the three code sites for the command's registration, action variant, and executor dispatch.
---

# Adding a new slash command

Three sites. Rust's exhaustive `match` on `SlashAction` enforces step 3
at compile time, so the only place a contributor can forget is the
filter test.

## 1. Add the command entry

File: `cli/src/tui/slash.rs` → `COMMANDS` constant

```rust
SlashCommand {
    name: "yourcmd",                          // lowercase ASCII (enforced by test)
    description: "Short, one-line description shown in the palette",
    action: SlashAction::YourAction,
},
```

Insert at the position you want it to appear in the palette (the array
order IS the display order). Convention: place by frequency / importance,
with `/quit` at the bottom.

## 2. Add the action variant

File: `cli/src/tui/slash.rs` → `SlashAction` enum

```rust
pub enum SlashAction {
    // ... existing variants ...
    YourAction,
}
```

## 3. Handle the variant in the executor

File: `cli/src/tui/slash.rs` → `execute_action`

```rust
SlashAction::YourAction => {
    // your effect here. examples:
    //   - inline state mutation: app.something = Some(...);
    //   - delegate to a module: super::your_module::execute(app, args)
    //   - return Action::Quit
    Action::Continue
}
```

Notes:
- `args: &str` carries inline arguments (`/yourcmd <args>`); use
  `parse_command_args` if you split them yourself elsewhere
- `close_palette(app)` runs at the top of `execute_action`, so the
  slash palette is already dismissed when your arm runs
- For commands that open a modal/picker, set the appropriate
  `Option<State>` field on `AppState`

## 4. Add tests

File: `cli/src/tests/tui/slash.rs`

The existing `filter_*` tests + `command_names_are_lowercase_ascii`
cover the data layer. Add an input-flow test in
`cli/src/tests/tui/input.rs` exercising the keystroke path:

```rust
#[test]
fn enter_runs_yourcmd_does_thing() {
    let mut app = test_app();
    type_str(&mut app, "/yourcmd");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    // assert side effect (app.something is Some, timeline contains note, etc.)
}
```

## 5. Update the help modal (optional)

File: `cli/src/tui/render/help_modal.rs`

If the new command is common enough to surface in the keybindings list,
update the `/` keybind description that lists known commands.

## Verify

```
cargo build && cargo clippy --all-targets -- -D warnings && cargo test
```

`SlashAction`'s exhaustive match fails the build if step 3 is missing.
