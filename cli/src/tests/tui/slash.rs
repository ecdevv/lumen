use super::*;

#[test]
fn filter_empty_returns_all_in_registration_order() {
    let r = filter_commands("/");
    assert_eq!(r.len(), COMMANDS.len());
    // Pin the registration order: help first.
    assert_eq!(r[0].name, "help");
}

#[test]
fn filter_bare_query_with_no_slash_treated_as_empty() {
    // We don't expect this in production (input always carries
    // the leading `/`) but the strip should still degrade
    // gracefully.
    let r = filter_commands("");
    assert_eq!(r.len(), COMMANDS.len());
}

#[test]
fn filter_prefix_match_narrows_results() {
    let r = filter_commands("/he");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "help");
}

#[test]
fn filter_is_case_insensitive() {
    let r = filter_commands("/HELP");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].name, "help");
}

#[test]
fn filter_no_match_returns_empty() {
    let r = filter_commands("/blarg");
    assert!(r.is_empty());
}

#[test]
fn is_slash_query_recognizes_single_line_starting_with_slash() {
    assert!(is_slash_query(&["/help".to_string()]));
    assert!(is_slash_query(&["/".to_string()]));
}

#[test]
fn is_slash_query_rejects_empty() {
    assert!(!is_slash_query(&[String::new()]));
    assert!(!is_slash_query(&[]));
}

#[test]
fn is_slash_query_rejects_multiline() {
    // A pasted multi-line buffer is not a command query even if
    // the first line starts with `/` - palette should close.
    assert!(!is_slash_query(&[
        "/help".to_string(),
        "extra".to_string(),
    ]));
}

#[test]
fn is_slash_query_rejects_no_leading_slash() {
    assert!(!is_slash_query(&["help".to_string()]));
}

#[test]
fn command_names_are_lowercase_ascii() {
    // Pins the invariant that `filter_commands` relies on. If you
    // add a command with uppercase or non-ASCII chars in `name`,
    // the filter's prefix-match will silently fail for that
    // command - so we catch it here at build time instead.
    for c in COMMANDS {
        assert!(
            !c.name.is_empty() && c.name.bytes().all(|b| b.is_ascii_lowercase()),
            "command name {:?} must be lowercase ASCII",
            c.name
        );
    }
}
