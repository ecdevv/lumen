use super::*;
use tempfile::tempdir;

/// Skip-if-not-installed: keeps `cargo test` green on machines
/// without ripgrep, even though production use requires it.
async fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok()
}

#[tokio::test]
async fn finds_matches_with_line_numbers() {
    if !rg_available().await {
        return;
    }
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "foo\nbar\nfoo bar\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.txt"), "no match here\n")
        .await
        .unwrap();

    let out = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"foo"}"#,
        )
        .await
        .unwrap();

    assert!(out.contains("a.txt:1:foo"), "got: {out}");
    assert!(out.contains("a.txt:3:foo bar"), "got: {out}");
    assert!(!out.contains("b.txt"), "got: {out}");
}

#[tokio::test]
async fn respects_case_insensitive() {
    if !rg_available().await {
        return;
    }
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "FOO\n").await.unwrap();

    let out = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"foo","case_insensitive":true}"#,
        )
        .await
        .unwrap();
    assert!(out.contains("a.txt:1:FOO"));
}

#[tokio::test]
async fn no_matches_returns_explicit_message() {
    if !rg_available().await {
        return;
    }
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "abc\n").await.unwrap();
    let out = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"zzz"}"#,
        )
        .await
        .unwrap();
    assert_eq!(out, "(no matches)");
}

#[tokio::test]
async fn invalid_regex_returns_tool_error() {
    if !rg_available().await {
        return;
    }
    let dir = tempdir().unwrap();
    let err = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"["}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[tokio::test]
async fn rejects_path_outside_cwd() {
    let dir = tempdir().unwrap();
    let err = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"x","path":"/etc"}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[tokio::test]
async fn missing_root_path_errors() {
    let dir = tempdir().unwrap();
    let err = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"x","path":"does/not/exist"}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(ref m) if m.contains("does not exist")));
}

// Snapshot pins the `path:line:content` framing emitted by `rg --json`
// + our consolidator. Lines are sorted before snapshotting because
// ripgrep's parallel directory walk is non-deterministic across
// runs - the snapshot pins format, not ordering. Ordering invariance
// is fine for the model since hits are independent.
#[tokio::test]
async fn snapshot_multi_file_matches() {
    if !rg_available().await {
        return;
    }
    let dir = tempdir().unwrap();
    tokio::fs::write(dir.path().join("a.txt"), "foo\nbar\nfoo bar\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.txt"), "foo at start\nstill foo\n")
        .await
        .unwrap();

    let out = GrepTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"pattern":"foo"}"#,
        )
        .await
        .unwrap();
    let mut lines: Vec<&str> = out.lines().collect();
    lines.sort_unstable();
    let sorted = lines.join("\n");
    insta::assert_snapshot!(sorted, @r"
    a.txt:1:foo
    a.txt:3:foo bar
    b.txt:1:foo at start
    b.txt:2:still foo
    ");
}
