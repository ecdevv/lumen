use super::*;
use crate::approval::AlwaysRejectGate;
use crate::config::AutoApply;
use std::sync::Arc;
use tempfile::tempdir;

fn ctx_rejecting() -> ToolContext {
    ToolContext::with_policy(
        std::env::temp_dir(),
        AutoApply::Never,
        Arc::new(AlwaysRejectGate),
    )
}

// --- gate integration ------------------------------------------- //

#[tokio::test]
async fn gate_reviews_every_shell_command_under_never() {
    // `Never` + AlwaysRejectGate: every command - even harmless
    // `echo` - is reviewed and the gate rejects. Absence of
    // execution markers (`exit:`, `--- stdout ---`) is the
    // load-bearing check (the command string itself shows up in
    // the rejection message and can't be asserted against).
    let result = ShellTool
        .invoke(&ctx_rejecting(), r#"{"command":"echo hello"}"#)
        .await
        .unwrap();
    assert!(
        result.contains("REJECTED by user"),
        "expected rejection, got: {result}"
    );
    assert!(
        result.contains("was NOT executed"),
        "rejection message should state non-execution, got: {result}"
    );
    assert!(
        !result.contains("exit:"),
        "command must not have executed (no exit line), got: {result}"
    );
    assert!(
        !result.contains("--- stdout ---"),
        "command must not have executed (no stdout block), got: {result}"
    );
}

#[tokio::test]
async fn gate_reviews_every_shell_command_under_safe() {
    // `Safe` also reviews every shell command (unlike Write/Edit
    // which auto-apply under Safe). The DESIGN.md "still prompt
    // for shell" promise is what this test pins.
    let ctx = ToolContext::with_policy(
        std::env::temp_dir(),
        AutoApply::Safe,
        Arc::new(AlwaysRejectGate),
    );
    let result = ShellTool
        .invoke(&ctx, r#"{"command":"ls"}"#)
        .await
        .unwrap();
    assert!(
        result.contains("REJECTED by user"),
        "Safe must still gate shell; got: {result}"
    );
    assert!(
        !result.contains("exit:"),
        "command must not have executed under Safe, got: {result}"
    );
}

// --- pre-existing unix shell tests below ------------------------ //

// These tests assume a Unix shell. Windows CI will need parallel
// tests using `cmd /C` semantics; deferred to when the Windows job
// job lands.
#[cfg(unix)]
#[tokio::test]
async fn captures_stdout() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"echo hello"}"#,
        )
        .await
        .unwrap();
    assert!(out.contains("exit: 0"));
    assert!(out.contains("hello"));
}

#[cfg(unix)]
#[tokio::test]
async fn nonzero_exit_is_reported() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"exit 7"}"#,
        )
        .await
        .unwrap();
    assert!(out.contains("exit: 7"));
    assert!(out.contains("failed"));
}

#[cfg(unix)]
#[tokio::test]
async fn captures_stderr() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"echo oops 1>&2"}"#,
        )
        .await
        .unwrap();
    assert!(out.contains("--- stderr ---"));
    assert!(out.contains("oops"));
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_long_command() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"sleep 5","timeout_seconds":1}"#,
        )
        .await
        .unwrap();
    assert!(out.contains("timed out"));
}

#[tokio::test]
async fn empty_command_errors() {
    let dir = tempdir().unwrap();
    let err = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"  "}"#,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

// Snapshot tests pin the exact `exit: N (failed)?` + `--- stdout ---`
// / `--- stderr ---` framing. The renderer's preview-line skipper
// keys off these markers (see `cli/src/tui/render` `preview()`), so a
// silent format change here would break the TUI preview heuristic.

#[cfg(unix)]
#[tokio::test]
async fn snapshot_success_with_stdout_and_stderr() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"echo out; echo err 1>&2"}"#,
        )
        .await
        .unwrap();
    insta::assert_snapshot!(out, @r"
    exit: 0
    --- stdout ---
    out
    --- stderr ---
    err
    ");
}

#[cfg(unix)]
#[tokio::test]
async fn snapshot_nonzero_exit_failure() {
    let dir = tempdir().unwrap();
    let out = ShellTool
        .invoke(
            &ToolContext::new(dir.path().to_path_buf()),
            r#"{"command":"echo before; exit 7"}"#,
        )
        .await
        .unwrap();
    insta::assert_snapshot!(out, @r"
    exit: 7 (failed)
    --- stdout ---
    before
    ");
}
