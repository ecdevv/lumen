use super::*;
use tokio::sync::mpsc::unbounded_channel;

fn gate_with(tx: UnboundedSender<UiMsg>, mode: AutoApply) -> TuiApprovalGate {
    TuiApprovalGate::new(tx, Arc::new(AtomicU8::new(mode.as_u8())))
}

#[tokio::test]
async fn review_diff_sends_request_and_returns_verdict() {
    let (tx, mut rx) = unbounded_channel();
    let gate = gate_with(tx, AutoApply::Never);

    // Drive both sides: ask in one task, respond from this task.
    let ask = tokio::spawn(async move {
        gate.review_diff(Path::new("f.rs"), "--- a/f.rs\n+++ b/f.rs\n")
            .await
    });

    let msg = rx.recv().await.expect("ui should receive request");
    let reply_tx = match msg {
        UiMsg::ApprovalRequest {
            kind: ApprovalKind::Diff { path, diff },
            reply,
        } => {
            assert_eq!(path, Path::new("f.rs"));
            assert!(diff.contains("--- a/f.rs"));
            reply
        }
        other => panic!("expected DiffApproval, got {other:?}"),
    };
    reply_tx.send(Verdict::Accept).unwrap();

    let verdict = ask.await.unwrap();
    assert_eq!(verdict, Verdict::Accept);
}

#[tokio::test]
async fn review_shell_sends_request_and_returns_verdict() {
    let (tx, mut rx) = unbounded_channel();
    let gate = gate_with(tx, AutoApply::Never);

    let ask = tokio::spawn(async move { gate.review_shell("rm -rf /").await });

    let msg = rx.recv().await.unwrap();
    let reply_tx = match msg {
        UiMsg::ApprovalRequest {
            kind: ApprovalKind::Shell { command },
            reply,
        } => {
            assert_eq!(command, "rm -rf /");
            reply
        }
        other => panic!("expected ShellApproval, got {other:?}"),
    };
    reply_tx.send(Verdict::Reject).unwrap();

    assert_eq!(ask.await.unwrap(), Verdict::Reject);
}

#[tokio::test]
async fn closed_ui_channel_returns_reject() {
    // Drop the receiver immediately so the send fails inside ask().
    let (tx, rx) = unbounded_channel::<UiMsg>();
    drop(rx);
    let gate = gate_with(tx, AutoApply::Never);
    assert_eq!(
        gate.review_diff(Path::new("f.rs"), "diff").await,
        Verdict::Reject
    );
}

#[tokio::test]
async fn dropped_reply_sender_returns_reject() {
    // UI receives the request and drops the reply channel
    // without sending - same shape as cancel_turn clearing
    // pending_approval mid-modal.
    let (tx, mut rx) = unbounded_channel();
    let gate = gate_with(tx, AutoApply::Never);

    let ask = tokio::spawn(async move {
        gate.review_shell("ls").await
    });

    let msg = rx.recv().await.unwrap();
    // Drop the reply sender without responding.
    drop(msg);

    assert_eq!(ask.await.unwrap(), Verdict::Reject);
}

// --- policy short-circuits (no prompt under permissive modes) -- //

#[tokio::test]
async fn safe_mode_auto_accepts_diff_without_prompting() {
    // Under Safe: edits auto-accept (the "accept edits" toggle).
    // The gate must NOT send a UiMsg and the verdict returns
    // immediately.
    let (tx, mut rx) = unbounded_channel::<UiMsg>();
    let gate = gate_with(tx, AutoApply::Safe);
    let verdict = gate.review_diff(Path::new("f.rs"), "diff").await;
    assert_eq!(verdict, Verdict::Accept);
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn safe_mode_still_prompts_for_shell() {
    // Safe is "auto edits, prompt dangerous shell" - the gate
    // is called from `needs_review(Safe, cmd) && looks_dangerous(cmd)`,
    // so when it IS called, the prompt must still run.
    let (tx, mut rx) = unbounded_channel::<UiMsg>();
    let gate = gate_with(tx, AutoApply::Safe);
    let ask = tokio::spawn(async move {
        gate.review_shell("rm -rf /").await
    });
    let msg = rx.recv().await.expect("gate should prompt under Safe");
    let reply_tx = match msg {
        UiMsg::ApprovalRequest { reply, .. } => reply,
        other => panic!("expected ApprovalRequest, got {other:?}"),
    };
    reply_tx.send(Verdict::Reject).unwrap();
    assert_eq!(ask.await.unwrap(), Verdict::Reject);
}
