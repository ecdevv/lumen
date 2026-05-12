use super::*;

#[test]
fn defaults_load_without_file() {
    let cfg = Config::load_from(None).unwrap();
    assert_eq!(cfg.provider.base_url, "http://localhost:8080");
    // Default is `Never`: prompt every edit / shell. Safer
    // out-of-box behavior; users opt into looser modes via the
    // Shift+Tab toggle or by setting `auto_apply` in config.toml.
    assert_eq!(cfg.auto_apply, AutoApply::Never);
}

#[test]
fn auto_apply_u8_round_trips() {
    for mode in [AutoApply::Never, AutoApply::Safe] {
        assert_eq!(AutoApply::from_u8(mode.as_u8()), mode);
    }
}

#[test]
fn auto_apply_from_u8_unknown_falls_back_to_never() {
    // Fail-safe-closed: a corrupted atomic byte must not
    // silently escalate the user's blast radius.
    assert_eq!(AutoApply::from_u8(255), AutoApply::Never);
}

#[test]
fn auto_apply_cycle_toggles_between_never_and_safe() {
    let mut m = AutoApply::Never;
    m = m.next();
    assert_eq!(m, AutoApply::Safe);
    m = m.next();
    assert_eq!(m, AutoApply::Never);
}

#[test]
fn auto_apply_round_trips_through_toml() {
    let cfg: Config = Figment::new()
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::string("auto_apply = \"never\""))
        .extract()
        .unwrap();
    assert_eq!(cfg.auto_apply, AutoApply::Never);
}

#[test]
fn debug_redacts_api_key_value() {
    let cfg = ProviderConfig {
        base_url: "http://x".into(),
        model: "y".into(),
        api_key: Some("super-secret-key-123".into()),
    };
    let s = format!("{cfg:?}");
    assert!(!s.contains("super-secret-key-123"), "api_key leaked: {s}");
    assert!(s.contains("redacted"), "expected redaction marker: {s}");
    // None case still shows None so absence is debuggable.
    let cfg2 = ProviderConfig {
        api_key: None,
        ..cfg
    };
    assert!(format!("{cfg2:?}").contains("None"));
}

#[test]
fn ui_auto_copy_on_select_defaults_on() {
    let cfg = Config::load_from(None).unwrap();
    assert!(cfg.ui.auto_copy_on_select);
}

#[test]
fn ui_auto_copy_on_select_can_be_disabled_via_toml() {
    let cfg: Config = Figment::new()
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::string(
            r"[ui]
auto_copy_on_select = false
",
        ))
        .extract()
        .unwrap();
    assert!(!cfg.ui.auto_copy_on_select);
}

#[test]
fn toml_string_overrides_provider_base_url() {
    let cfg: Config = Figment::new()
        .merge(Serialized::defaults(Config::default()))
        .merge(Toml::string(
            r#"[provider]
base_url = "http://example.com:9999"
"#,
        ))
        .extract()
        .unwrap();
    assert_eq!(cfg.provider.base_url, "http://example.com:9999");
}

#[test]
fn set_model_in_file_creates_new_file_with_minimal_stanza() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    assert!(!path.exists());
    Config::set_model_in_file(&path, "qwen2.5-coder").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[provider]"), "provider table written");
    assert!(
        content.contains("model = \"qwen2.5-coder\""),
        "model key written"
    );
}

#[test]
fn set_model_in_file_overwrites_existing_model_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[provider]\nmodel = \"old-model\"\nbase_url = \"http://localhost:8080\"\n",
    )
    .unwrap();
    Config::set_model_in_file(&path, "new-model").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("model = \"new-model\""));
    assert!(!content.contains("\"old-model\""));
    // Other keys preserved (no full rewrite).
    assert!(content.contains("base_url = \"http://localhost:8080\""));
}

#[test]
fn set_model_in_file_preserves_comments_and_ordering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let original = "# top-level comment\n\
                    [provider]\n\
                    # inline comment before model\n\
                    model = \"old\"\n\
                    base_url = \"http://localhost:8080\"\n";
    std::fs::write(&path, original).unwrap();
    Config::set_model_in_file(&path, "new").unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("# top-level comment"), "top comment survived");
    assert!(
        content.contains("# inline comment before model"),
        "inline comment survived"
    );
    assert!(content.contains("model = \"new\""));
    // Ordering preserved: model still appears before base_url.
    let m_idx = content.find("model = \"new\"").unwrap();
    let b_idx = content.find("base_url").unwrap();
    assert!(m_idx < b_idx, "key order preserved");
}

#[test]
fn set_model_in_file_creates_parent_directory_if_missing() {
    let dir = tempfile::tempdir().unwrap();
    // Nested path that doesn't exist yet.
    let path = dir.path().join("subdir/inner/config.toml");
    Config::set_model_in_file(&path, "qwen").unwrap();
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("model = \"qwen\""));
}

#[test]
fn write_template_creates_file_with_header_and_sections() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = Config::default();
    Config::write_template_to(&path, &cfg).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    // Header comment is present.
    assert!(content.starts_with("# Lumen configuration."));
    // Each section header is written.
    assert!(content.contains("[provider]"));
    assert!(content.contains("[ui]"));
    // Documented top-level key.
    assert!(content.contains("auto_apply ="));
}

#[test]
fn write_template_round_trips_default_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = Config::default();
    Config::write_template_to(&path, &cfg).unwrap();

    // Loading the templated file back yields the same default
    // values - i.e. the template doesn't accidentally write
    // values that differ from `Config::default()`.
    let loaded = Config::load_from(Some(&path)).unwrap();
    assert_eq!(loaded.provider.model, cfg.provider.model);
    assert_eq!(loaded.provider.base_url, cfg.provider.base_url);
    assert_eq!(loaded.ui.auto_copy_on_select, cfg.ui.auto_copy_on_select);
    assert_eq!(loaded.ui.unicode_glyphs, cfg.ui.unicode_glyphs);
    assert_eq!(loaded.auto_apply, cfg.auto_apply);
}

#[test]
fn write_template_creates_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/inner/config.toml");
    Config::write_template_to(&path, &Config::default()).unwrap();
    assert!(path.exists());
}

#[test]
fn write_template_then_surgical_set_in_file_works() {
    // The two write paths must compose: a templated file is
    // valid input for `set_in_file`. Catches regressions where
    // the template emits a shape the toml_edit parser rejects.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::write_template_to(&path, &Config::default()).unwrap();
    Config::set_model_in_file(&path, "post-template-model").unwrap();
    let loaded = Config::load_from(Some(&path)).unwrap();
    assert_eq!(loaded.provider.model, "post-template-model");
    // Header comment survives the surgical write.
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.starts_with("# Lumen configuration."));
}
