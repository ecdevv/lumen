use super::*;
use crate::approval::AlwaysRejectGate;
use crate::config::AutoApply;
use crate::error::Error;
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
async fn writes_file_creating_parents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested/dir/out.txt");

    WriteTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &format!(
                r#"{{"path":"{}","content":"hello"}}"#,
                path.display()
            ),
        )
        .await
        .unwrap();

    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "hello");
}

#[tokio::test]
async fn rejects_path_outside_cwd() {
    let dir = tempdir().unwrap();
    let err = WriteTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            r#"{"path":"/tmp/escape.txt","content":"x"}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[tokio::test]
async fn gate_rejection_leaves_file_unchanged() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.txt");
    tokio::fs::write(&path, "original").await.unwrap();

    let result = WriteTool
        .invoke(
            &ctx_rejecting(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "content": "new content",
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
        "original",
        "file must be unchanged when gate rejects"
    );
}

#[tokio::test]
async fn writing_identical_content_is_noop_no_gate_call() {
    // No-change path: original bytes == proposed bytes. Should
    // skip gate entirely (nothing to review) and not touch the
    // file's mtime. Using AlwaysRejectGate to prove the gate
    // was bypassed: if it had been consulted, the rejection
    // would have produced "user rejected" instead.
    let dir = tempdir().unwrap();
    let path = dir.path().join("f.txt");
    tokio::fs::write(&path, "same").await.unwrap();

    let result = WriteTool
        .invoke(
            &ctx_rejecting(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "content": "same",
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert!(
        result.contains("no change"),
        "expected no-change message, got: {result}"
    );
}

#[tokio::test]
async fn new_file_write_through_default_gate_succeeds() {
    // Default ctx uses AutoAcceptGate; write should land.
    // Sanity check that the gate-wired path doesn't regress
    // the new-file case.
    let dir = tempdir().unwrap();
    let path = dir.path().join("brand_new.txt");
    WriteTool
        .invoke(
            &ctx(dir.path().to_path_buf()),
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "content": "hello",
            })
            .to_string(),
        )
        .await
        .unwrap();
    assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "hello");
}
