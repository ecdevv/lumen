use super::*;
use std::sync::Arc;

#[tokio::test]
async fn auto_accept_gate_accepts_diff() {
    let gate = AutoAcceptGate;
    assert_eq!(
        gate.review_diff(Path::new("a.rs"), "--- a\n+++ b\n").await,
        Verdict::Accept
    );
}

#[tokio::test]
async fn auto_accept_gate_accepts_shell() {
    let gate = AutoAcceptGate;
    assert_eq!(gate.review_shell("rm -rf /").await, Verdict::Accept);
}

/// Mock that always rejects. Confirms the trait object path
/// works end-to-end, so step 10's TUI gate has a clear shape
/// to slot into.
#[derive(Debug)]
struct AlwaysReject;

#[async_trait]
impl ApprovalGate for AlwaysReject {
    async fn review_diff(&self, _: &Path, _: &str) -> Verdict {
        Verdict::Reject
    }
    async fn review_shell(&self, _: &str) -> Verdict {
        Verdict::Reject
    }
}

#[tokio::test]
async fn custom_gate_routes_through_trait_object() {
    let gate: Arc<dyn ApprovalGate> = Arc::new(AlwaysReject);
    assert_eq!(
        gate.review_diff(Path::new("x"), "").await,
        Verdict::Reject
    );
    assert_eq!(gate.review_shell("ls").await, Verdict::Reject);
}
