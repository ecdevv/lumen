use super::*;

#[test]
fn filter_empty_loaded_returns_all() {
    let status = ModelPickerStatus::Loaded {
        models: vec!["a".into(), "b".into(), "c".into()],
    };
    let r = filter_models(&status, "");
    assert_eq!(r, vec!["a", "b", "c"]);
}

#[test]
fn filter_substring_match_is_case_insensitive() {
    let status = ModelPickerStatus::Loaded {
        models: vec!["Qwen2.5-coder-32b".into(), "llama-3.1-8b".into()],
    };
    let r = filter_models(&status, "QWEN");
    assert_eq!(r, vec!["Qwen2.5-coder-32b"]);
}

#[test]
fn filter_substring_matches_middle_of_name() {
    // The slash filter is prefix; the model filter is substring -
    // model names like "Qwen2.5-coder-32b-instruct" let users
    // type "coder" to find them, not just the leading token.
    let status = ModelPickerStatus::Loaded {
        models: vec!["qwen2.5-coder-32b".into(), "llama-3.1-8b".into()],
    };
    let r = filter_models(&status, "coder");
    assert_eq!(r, vec!["qwen2.5-coder-32b"]);
}

#[test]
fn filter_loading_state_returns_empty() {
    let status = ModelPickerStatus::Loading;
    let r = filter_models(&status, "qwen");
    assert!(r.is_empty());
}

#[test]
fn filter_error_state_returns_empty() {
    let status = ModelPickerStatus::Error {
        message: "boom".into(),
    };
    let r = filter_models(&status, "");
    assert!(r.is_empty());
}

#[test]
fn loading_constructor_has_zero_selected_and_loading_status() {
    let p = ModelPickerState::loading();
    assert_eq!(p.selected, 0);
    assert!(matches!(p.status, ModelPickerStatus::Loading));
}
