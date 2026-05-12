use super::*;

#[test]
fn parse_auto_apply_accepts_known_values() {
    assert_eq!(parse_auto_apply("never").unwrap(), AutoApply::Never);
    assert_eq!(parse_auto_apply("safe").unwrap(), AutoApply::Safe);
}

#[test]
fn parse_auto_apply_rejects_removed_always_value() {
    // `always` was removed alongside the runtime "auto-all" tier;
    // the parser must surface a clear error instead of silently
    // mapping it to something else.
    let err = parse_auto_apply("always").unwrap_err();
    assert!(err.contains("always"));
}

#[test]
fn parse_auto_apply_rejects_unknown() {
    let err = parse_auto_apply("yolo").unwrap_err();
    assert!(err.contains("yolo"));
}

#[test]
fn cli_parses_with_no_args() {
    let cli = Cli::try_parse_from(["lumen"]).unwrap();
    assert!(cli.command.is_none());
    assert!(cli.config.is_none());
}

#[test]
fn cli_parses_global_flags_before_subcommand() {
    let cli = Cli::try_parse_from([
        "lumen", "--model", "qwen2.5-coder:7b", "sessions", "ls",
    ])
    .unwrap();
    assert_eq!(cli.model.as_deref(), Some("qwen2.5-coder:7b"));
    assert!(matches!(
        cli.command,
        Some(Command::Sessions {
            action: SessionsAction::Ls
        })
    ));
}

#[test]
fn cli_parses_global_flags_after_subcommand() {
    let cli = Cli::try_parse_from([
        "lumen",
        "sessions",
        "rm",
        "abc",
        "--auto-apply",
        "never",
    ])
    .unwrap();
    assert_eq!(cli.auto_apply, Some(AutoApply::Never));
    match cli.command {
        Some(Command::Sessions {
            action: SessionsAction::Rm { id },
        }) => assert_eq!(id, "abc"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn cli_rejects_invalid_auto_apply() {
    let err = Cli::try_parse_from(["lumen", "--auto-apply", "yolo"]).unwrap_err();
    assert!(err.to_string().contains("yolo"));
}

#[test]
fn load_config_uses_default_path_when_no_cli_flag() {
    // Regression: without `--config`, the CLI must fall back to the
    // XDG default. Pre-fix, `load_from(None)` skipped the file
    // layer entirely and silently dropped every user setting.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    std::fs::write(
        &cfg_path,
        "[ui]\nauto_copy_on_select = true\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["lumen"]).unwrap();
    let cfg = load_config_from(&cli, Some(cfg_path)).unwrap();
    assert!(
        cfg.ui.auto_copy_on_select,
        "default config path was not loaded"
    );
}

#[test]
fn load_config_cli_path_overrides_default() {
    let dir = tempfile::tempdir().unwrap();
    let default_path = dir.path().join("default.toml");
    std::fs::write(
        &default_path,
        "[ui]\nauto_copy_on_select = false\n",
    )
    .unwrap();
    let cli_path = dir.path().join("cli.toml");
    std::fs::write(
        &cli_path,
        "[ui]\nauto_copy_on_select = true\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from([
        "lumen",
        "--config",
        cli_path.to_str().unwrap(),
    ])
    .unwrap();
    let cfg = load_config_from(&cli, Some(default_path)).unwrap();
    assert!(
        cfg.ui.auto_copy_on_select,
        "--config should win over default path"
    );
}

#[test]
fn load_config_no_file_anywhere_uses_compiled_defaults() {
    // Container-style env: no XDG resolution (`default_path = None`),
    // no `--config` flag. Must succeed with built-in defaults.
    let cli = Cli::try_parse_from(["lumen"]).unwrap();
    let cfg = load_config_from(&cli, None).unwrap();
    // `auto_copy_on_select` defaults to `true` (UiConfig::default).
    assert!(cfg.ui.auto_copy_on_select);
}

#[test]
fn cli_flag_wins_against_compiled_default_when_no_other_source() {
    // Layering invariant proven without mutating process env:
    // CLI flag overrides are the final step of load_config_from,
    // so they win over whatever Config::load_from returned -
    // regardless of source (file / env / compiled default).
    // The file path is covered by
    // `load_config_cli_flag_overrides_file_setting`; this is
    // the no-file, no-env-mutation companion that exercises
    // the override step against the compiled default.
    //
    // A direct `LUMEN_PROVIDER__MODEL=...` env-mutation test
    // would have to serialize against every other
    // config-loading test in this module (env vars are
    // process-global and cargo test runs in parallel). Not
    // worth the suite-wide mutex for what figment already
    // tests upstream - env and file are peer sources, so
    // override-vs-file transitively proves override-vs-env.
    let cli = Cli::try_parse_from(["lumen", "--model", "from-flag"]).unwrap();
    let cfg = load_config_from(&cli, None).unwrap();
    assert_eq!(cfg.provider.model, "from-flag");
}

#[test]
fn load_config_cli_flag_overrides_file_setting() {
    // Layer ordering: file -> CLI flag, with the flag winning.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.toml");
    std::fs::write(
        &cfg_path,
        "[provider]\nmodel = \"from-file\"\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["lumen", "--model", "from-flag"]).unwrap();
    let cfg = load_config_from(&cli, Some(cfg_path)).unwrap();
    assert_eq!(cfg.provider.model, "from-flag");
}
