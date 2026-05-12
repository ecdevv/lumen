---
name: add-config-field
description: Use when the user asks to add a new configuration field to lumen (e.g., "add a config field for X", "add a new setting called Y", "expose Z in the config"). Walks through the four code sites that must be updated so the field round-trips through compiled defaults, file load, /settings UI, and TOML write-back.
---

# Adding a new editable config field

Four files. The exhaustive-match on `Field` enforces step 3 at compile
time, but the others rely on the contributor remembering. This skill is
that reminder.

## 1. Add the field to `Config` (or a sub-struct)

File: `core/src/config.rs`

- Pick the right struct (`Config`, `ProviderConfig`, `UiConfig`, etc.)
- Add the field with `#[serde(default)]` semantics if non-default
- Update the `Default` impl if the field's default isn't `Default::default()`
- If the field is a new enum, derive `Serialize`, `Deserialize`,
  `PartialEq`, `Eq`, `Copy`, `Clone`, `Default` as appropriate. Use
  `#[serde(rename_all = "lowercase")]` if the on-disk form should be
  lower-case
- If sensitive (e.g., another secret like `api_key`), update the manual
  `Debug` impl on its containing struct to redact

## 2. Update the first-launch template

File: `core/src/config.rs` → `Config::write_template_to`

- Add a section to the hand-coded template literal so first-launch
  users see the new key + comment in their seeded `config.toml`
- Comment should explain what the field controls + when to change it
- Use `toml_basic_escape` for any String value that might contain
  `\` or `"`
- If the field gates a non-essential feature, document the default
  rationale in the comment

## 3. (User-facing only) Add to the `Field` enum

File: `cli/src/tui/settings.rs`

Skip if the field should be file-only (not in `/settings` UI - e.g.,
paths, logging level, telemetry flags).

If user-facing:

- Add a `Field::YourField` variant
- Add it to `Field::ALL` in the desired display order
- Implement the methods (Rust's exhaustive match makes this mechanical
  - the build fails on each unhandled arm):
  - `section()` - section label ("Provider" / "UI" / "Approval" / new)
  - `label()` - short snake_case name shown left of the value
  - `kind()` - `Text` / `Bool` / `Enum { options }`
  - `sensitive()` - `true` if value should be `<redacted>` in display
  - `toml_path()` - `(Some("section"), "key")` or `(None, "key")`
    for top-level
  - `read(&Config) -> String` - extract display string
  - `apply(&mut Config, &str) -> Result<(), String>` - validate +
    write back to cfg
  - `to_toml_item(&str) -> toml_edit::Item` - serialize for the
    config file

## 4. Add tests

File: `cli/src/tests/tui/settings.rs`

The existing tests (`read_round_trips_text_fields_through_apply`,
`apply_bool_accepts_only_true_or_false`, etc.) iterate `Field::ALL`,
so most behavior is covered automatically. If the field needs custom
validation, add a dedicated test for the success + failure cases.

## Verify

```
cargo build && cargo clippy --all-targets -- -D warnings && cargo test
```

The exhaustive-match failures in step 3 surface any forgotten methods.
The round-trip tests catch wiring mistakes between `apply` and `read`.
