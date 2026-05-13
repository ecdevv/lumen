use super::*;
use lumen_core::{AutoApply, Config};

#[test]
fn all_fields_have_distinct_labels() {
    let mut seen = std::collections::HashSet::new();
    for f in Field::ALL {
        assert!(seen.insert(f.label()), "duplicate label: {}", f.label());
    }
}

#[test]
fn read_round_trips_text_fields_through_apply() {
    let mut cfg = Config::default();
    for &field in Field::ALL {
        if !matches!(field.kind(), FieldKind::Text) {
            continue;
        }
        let new_value = format!("test-{}", field.label());
        field.apply(&mut cfg, &new_value).unwrap();
        assert_eq!(field.read(&cfg), new_value, "round-trip for {}", field.label());
    }
}

#[test]
fn apply_bool_accepts_only_true_or_false() {
    let mut cfg = Config::default();
    Field::UiAutoCopyOnSelect.apply(&mut cfg, "true").unwrap();
    assert!(cfg.ui.auto_copy_on_select);
    Field::UiAutoCopyOnSelect.apply(&mut cfg, "false").unwrap();
    assert!(!cfg.ui.auto_copy_on_select);
    assert!(Field::UiAutoCopyOnSelect.apply(&mut cfg, "yes").is_err());
}

#[test]
fn apply_enum_accepts_only_known_variants() {
    let mut cfg = Config::default();
    Field::AutoApply.apply(&mut cfg, "safe").unwrap();
    assert_eq!(cfg.auto_apply, AutoApply::Safe);
    Field::AutoApply.apply(&mut cfg, "never").unwrap();
    assert_eq!(cfg.auto_apply, AutoApply::Never);
    assert!(Field::AutoApply.apply(&mut cfg, "yolo").is_err());
}

#[test]
fn cycle_next_wraps_at_end_of_enum_options() {
    assert_eq!(Field::AutoApply.cycle_next("never"), Some("safe"));
    assert_eq!(Field::AutoApply.cycle_next("safe"), Some("never"));
    // Unknown current value falls back to the first option.
    assert_eq!(Field::AutoApply.cycle_next("yolo"), Some("safe"));
}

#[test]
fn cycle_next_returns_none_for_non_enum_field() {
    assert!(Field::ProviderModel.cycle_next("anything").is_none());
    assert!(Field::UiAutoCopyOnSelect.cycle_next("true").is_none());
}

#[test]
fn api_key_round_trips_set_and_unset() {
    let mut cfg = Config::default();
    Field::ProviderApiKey.apply(&mut cfg, "sk-test").unwrap();
    assert_eq!(cfg.provider.api_key, "sk-test");
    Field::ProviderApiKey.apply(&mut cfg, "").unwrap();
    assert!(cfg.provider.api_key.is_empty(), "empty input clears the key");
}

#[test]
fn apply_provider_base_url_rejects_empty() {
    let mut cfg = Config::default();
    assert!(Field::ProviderBaseUrl.apply(&mut cfg, "  ").is_err());
}

#[test]
fn to_toml_item_for_bool_writes_native_bool() {
    let item = Field::UiAutoCopyOnSelect.to_toml_item("true");
    // toml_edit::Value::Boolean prints as `true` (no quotes).
    assert_eq!(item.to_string().trim(), "true");
}
