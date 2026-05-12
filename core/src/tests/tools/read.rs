use super::*;
use std::path::PathBuf;
use tempfile::tempdir;

fn ctx(p: PathBuf) -> ToolContext {
    ToolContext::new(p)
}

#[tokio::test]
async fn reads_file_with_line_numbers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    tokio::fs::write(&path, "alpha\nbeta\ngamma\n").await.unwrap();

    let out = ReadTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &format!(r#"{{"path":"{}"}}"#, path.display()),
        )
        .await
        .unwrap();

    assert!(out.contains("     1\talpha"));
    assert!(out.contains("     2\tbeta"));
    assert!(out.contains("     3\tgamma"));
}

#[tokio::test]
async fn honors_offset_and_limit() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("h.txt");
    tokio::fs::write(&path, "a\nb\nc\nd\n").await.unwrap();

    let out = ReadTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &format!(r#"{{"path":"{}","offset":1,"limit":2}}"#, path.display()),
        )
        .await
        .unwrap();

    assert!(out.contains("     2\tb"));
    assert!(out.contains("     3\tc"));
    assert!(!out.contains("     1\ta"));
    assert!(!out.contains("     4\td"));
    // Truncation message format pin: the model uses the `offset: N`
    // hint to continue paging. Format drift would break that affordance.
    assert!(out.contains("... (1 more lines; pass `offset: 3` to continue)"));
}

#[tokio::test]
async fn rejects_path_outside_cwd() {
    let dir = tempdir().unwrap();
    let err = ReadTool
        .invoke(&ctx(dir.path().to_path_buf()), r#"{"path":"/etc/passwd"}"#)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

// Snapshot tests pin the exact `{width:>6}\t{line}` framing the
// model receives. Tab-prefixed line numbers were chosen for parser
// stability across providers; any drift in that format would silently
// degrade in-context citing.

#[tokio::test]
async fn snapshot_full_file_read() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("snippet.rs");
    tokio::fs::write(
        &path,
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .await
    .unwrap();

    let out = ReadTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &format!(r#"{{"path":"{}"}}"#, path.display()),
        )
        .await
        .unwrap();
    // Indent 4 (matching the closing `"`) gets dedented away;
    // padding-width is pinned by the granular `contains("     1\talpha")`
    // tests above. This snapshot pins structural shape: line-number,
    // tab, content.
    insta::assert_snapshot!(out, @"
    1\tfn main() {
    2\t    println!(\"hi\");
    3\t}
    ");
}

