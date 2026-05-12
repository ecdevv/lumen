use super::*;

#[test]
fn builtins_are_registered_in_alphabetical_order() {
    let r = ToolRegistry::with_builtins();
    let names: Vec<_> = r.schemas().into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["edit", "grep", "read", "shell", "write"]);
}

#[tokio::test]
async fn unknown_tool_returns_tool_error() {
    let r = ToolRegistry::with_builtins();
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let err = r.invoke(&ctx, "nope", "{}").await.unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}
