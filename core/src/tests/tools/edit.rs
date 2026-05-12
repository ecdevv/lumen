use super::*;
use crate::approval::AlwaysRejectGate;
use crate::config::AutoApply;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

fn ctx(p: PathBuf) -> ToolContext {
    ToolContext::new(p)
}

fn ctx_rejecting(p: PathBuf) -> ToolContext {
    ToolContext::with_policy(p, AutoApply::Never, Arc::new(AlwaysRejectGate))
}

#[tokio::test]
async fn replaces_unique_occurrence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.rs");
    tokio::fs::write(&path, "let x = 1;\nlet y = 2;\n").await.unwrap();

    EditTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "old_string": "let x = 1;",
                "new_string": "let x = 42;"
            })
            .to_string(),
        )
        .await
        .unwrap();

    let after = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(after, "let x = 42;\nlet y = 2;\n");
}

#[tokio::test]
async fn errors_when_old_string_missing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.rs");
    tokio::fs::write(&path, "abc").await.unwrap();

    let err = EditTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "old_string": "zzz",
                "new_string": "x"
            })
            .to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(ref m) if m.contains("not found")));
}

#[tokio::test]
async fn errors_when_old_string_not_unique() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.rs");
    tokio::fs::write(&path, "x x x").await.unwrap();

    let err = EditTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "old_string": "x",
                "new_string": "y"
            })
            .to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(ref m) if m.contains("not unique")));
}

#[tokio::test]
async fn gate_rejection_leaves_file_unchanged() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.rs");
    let original = "let x = 1;\nlet y = 2;\n";
    tokio::fs::write(&path, original).await.unwrap();

    let result = EditTool
        .invoke(
            &ctx_rejecting(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "old_string": "let x = 1;",
                "new_string": "let x = 42;"
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(
        result.contains("REJECTED by user"),
        "expected rejection message, got: {result}"
    );
    assert!(
        result.contains("File unchanged"),
        "rejection message should state file unchanged, got: {result}"
    );
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        original,
        "file must be unchanged when gate rejects"
    );
}

#[tokio::test]
async fn replace_all_replaces_every_occurrence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.rs");
    tokio::fs::write(&path, "x x x").await.unwrap();

    EditTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "old_string": "x",
                "new_string": "y",
                "replace_all": true
            })
            .to_string(),
        )
        .await
        .unwrap();

    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "y y y");
}
