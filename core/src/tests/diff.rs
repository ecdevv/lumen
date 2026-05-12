use super::*;
use std::path::PathBuf;

#[test]
fn identical_inputs_produce_empty_diff() {
    let d = unified_diff("abc\n", "abc\n", &PathBuf::from("f.rs"));
    assert!(d.is_empty(), "no-change diff must be empty, got: {d:?}");
}

#[test]
fn single_line_change_shows_minus_plus() {
    let d = unified_diff(
        "let x = 1;\nlet y = 2;\n",
        "let x = 42;\nlet y = 2;\n",
        &PathBuf::from("f.rs"),
    );
    assert!(d.contains("--- a/f.rs"));
    assert!(d.contains("+++ b/f.rs"));
    assert!(d.contains("-let x = 1;"));
    assert!(d.contains("+let x = 42;"));
    // Unchanged context line survives as a space-prefixed row.
    assert!(d.contains(" let y = 2;"));
}

#[test]
fn new_file_shows_all_lines_as_added() {
    let d = unified_diff(
        "",
        "alpha\nbeta\n",
        &PathBuf::from("created.txt"),
    );
    assert!(d.contains("+alpha"));
    assert!(d.contains("+beta"));
    // No removed lines (it's a new file).
    assert!(!d.lines().any(|l| l.starts_with('-') && !l.starts_with("---")));
}

#[test]
fn deletion_shows_minus_lines() {
    let d = unified_diff(
        "keep\nremove\nkeep2\n",
        "keep\nkeep2\n",
        &PathBuf::from("f.txt"),
    );
    assert!(d.contains("-remove"));
    assert!(!d.lines().any(|l| l.starts_with('+') && !l.starts_with("+++")));
}

#[test]
fn hunk_header_present_for_real_changes() {
    let d = unified_diff(
        "a\nb\nc\n",
        "a\nB\nc\n",
        &PathBuf::from("f.txt"),
    );
    assert!(
        d.lines().any(|l| l.starts_with("@@")),
        "expected hunk header in: {d}"
    );
}

// Snapshot tests below pin the exact unified-diff format so a
// `similar`-version bump or accidental header change surfaces as a
// review-able snapshot diff rather than silently shifting what the
// model and approval modal see. Granular tests above cover invariants
// (empty-on-identical, hunk-header-present); these cover *shape*.

#[test]
fn snapshot_single_line_change() {
    let d = unified_diff(
        "let x = 1;\nlet y = 2;\nlet z = 3;\n",
        "let x = 42;\nlet y = 2;\nlet z = 3;\n",
        &PathBuf::from("f.rs"),
    );
    insta::assert_snapshot!(d, @r"
    --- a/f.rs
    +++ b/f.rs
    @@ -1,3 +1,3 @@
    -let x = 1;
    +let x = 42;
     let y = 2;
     let z = 3;
    ");
}

#[test]
fn snapshot_new_file() {
    let d = unified_diff(
        "",
        "alpha\nbeta\ngamma\n",
        &PathBuf::from("created.txt"),
    );
    insta::assert_snapshot!(d, @r"
    --- a/created.txt
    +++ b/created.txt
    @@ -0,0 +1,3 @@
    +alpha
    +beta
    +gamma
    ");
}

#[test]
fn snapshot_multi_hunk() {
    // Two changes separated by enough unchanged context that
    // `similar` emits them as separate `@@` hunks rather than
    // one merged block.
    let old = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut new_lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
    new_lines[1] = "line 1 CHANGED".to_string();
    new_lines[18] = "line 18 CHANGED".to_string();
    let new = new_lines.join("\n");
    let d = unified_diff(&old, &new, &PathBuf::from("multi.txt"));
    insta::assert_snapshot!(d, @r"
    --- a/multi.txt
    +++ b/multi.txt
    @@ -1,5 +1,5 @@
     line 0
    -line 1
    +line 1 CHANGED
     line 2
     line 3
     line 4
    @@ -16,5 +16,5 @@
     line 15
     line 16
     line 17
    -line 18
    +line 18 CHANGED
     line 19
    \ No newline at end of file
    ");
}
