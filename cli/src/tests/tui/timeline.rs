use super::*;
use lumen_core::provider::FinishReason;

#[test]
fn new_timeline_is_empty() {
    let t = Timeline::new();
    assert!(t.is_empty());
    assert_eq!(t.items().len(), 0);
}

#[test]
fn push_user_appends_user_item() {
    let mut t = Timeline::new();
    t.push_user("hello".into());
    assert_eq!(t.items(), &[TimelineItem::User("hello".into())]);
}

#[test]
fn streamed_text_chunks_coalesce_into_one_block() {
    let mut t = Timeline::new();
    t.push_assistant_text("Hello, ".into());
    t.push_assistant_text("world!".into());
    assert_eq!(
        t.items(),
        &[TimelineItem::AssistantText("Hello, world!".into())]
    );
}

#[test]
fn assistant_text_after_tool_call_starts_new_block() {
    let mut t = Timeline::new();
    t.push_assistant_text("First. ".into());
    t.push_tool_call("c1".into(), "read".into(), "{}".into());
    t.push_assistant_text("Second.".into());

    let items = t.items();
    assert_eq!(items.len(), 3);
    assert!(matches!(&items[0], TimelineItem::AssistantText(s) if s == "First. "));
    assert!(matches!(&items[1], TimelineItem::ToolCall { id, .. } if id == "c1"));
    assert!(matches!(&items[2], TimelineItem::AssistantText(s) if s == "Second."));
}

#[test]
fn finish_tool_call_updates_status_to_done() {
    let mut t = Timeline::new();
    t.push_tool_call("c1".into(), "read".into(), r#"{"path":"a"}"#.into());
    t.finish_tool_call("c1", "32 lines".into(), false);

    match &t.items()[0] {
        TimelineItem::ToolCall { status, .. } => {
            assert_eq!(*status, ToolStatus::Done("32 lines".into()));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn finish_tool_call_records_elapsed_duration() {
    let mut t = Timeline::new();
    t.push_tool_call("c1".into(), "read".into(), "{}".into());
    // Brief sleep so elapsed is non-zero.
    std::thread::sleep(Duration::from_millis(2));
    t.finish_tool_call("c1", "ok".into(), false);

    match &t.items()[0] {
        TimelineItem::ToolCall { elapsed, .. } => {
            let d = elapsed.expect("elapsed should be set after finish");
            assert!(d >= Duration::from_millis(1));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn running_tool_call_has_no_elapsed() {
    let mut t = Timeline::new();
    t.push_tool_call("c1".into(), "read".into(), "{}".into());
    match &t.items()[0] {
        TimelineItem::ToolCall { elapsed, .. } => {
            assert!(elapsed.is_none());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn finish_tool_call_with_error_flag_marks_error() {
    let mut t = Timeline::new();
    t.push_tool_call("c1".into(), "read".into(), "{}".into());
    t.finish_tool_call("c1", "bad path".into(), true);

    match &t.items()[0] {
        TimelineItem::ToolCall { status, .. } => {
            assert_eq!(*status, ToolStatus::Error("bad path".into()));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn finish_unknown_id_is_noop() {
    let mut t = Timeline::new();
    t.push_tool_call("c1".into(), "read".into(), "{}".into());
    t.finish_tool_call("nonexistent", "x".into(), false);

    match &t.items()[0] {
        TimelineItem::ToolCall { status, .. } => {
            assert_eq!(*status, ToolStatus::Running);
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn finish_picks_latest_when_ids_collide() {
    // Defensive: if a buggy provider reuses an id, latest wins.
    let mut t = Timeline::new();
    t.push_tool_call("c".into(), "read".into(), "{}".into());
    t.push_tool_call("c".into(), "write".into(), "{}".into());
    t.finish_tool_call("c", "ok".into(), false);

    match &t.items()[0] {
        TimelineItem::ToolCall { status, .. } => {
            assert_eq!(*status, ToolStatus::Running, "first call untouched");
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
    match &t.items()[1] {
        TimelineItem::ToolCall { status, .. } => {
            assert_eq!(*status, ToolStatus::Done("ok".into()));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn apply_assistant_text_event_does_not_end_turn() {
    let mut t = Timeline::new();
    let ended = t.apply(AgentEvent::AssistantText("hi".into()));
    assert!(!ended);
    assert_eq!(
        t.items(),
        &[TimelineItem::AssistantText("hi".into())]
    );
}

#[test]
fn apply_tool_lifecycle_via_events() {
    let mut t = Timeline::new();
    t.apply(AgentEvent::ToolCallStart {
        id: "c1".into(),
        name: "read".into(),
        arguments: r#"{"p":"a"}"#.into(),
    });
    t.apply(AgentEvent::ToolCallEnd {
        id: "c1".into(),
        result: "ok".into(),
        is_error: false,
    });

    assert!(matches!(
        &t.items()[0],
        TimelineItem::ToolCall {
            status: ToolStatus::Done(s),
            ..
        } if s == "ok"
    ));
}

#[test]
fn apply_turn_end_returns_true() {
    let mut t = Timeline::new();
    let ended = t.apply(AgentEvent::TurnEnd {
        reason: FinishReason::Stop,
    });
    assert!(ended);
}

#[test]
fn turn_end_after_user_emits_duration_footer() {
    let mut t = Timeline::new();
    t.push_user("hello".into());
    std::thread::sleep(Duration::from_millis(2));
    t.apply(AgentEvent::TurnEnd {
        reason: FinishReason::Stop,
    });
    let last = t.items().last().expect("expected footer note");
    match last {
        TimelineItem::Note(s) => {
            assert!(s.contains(" for "), "expected verb-for-duration shape: {s}");
            // Verb is one of our keyed pool; duration is the small
            // human-readable shape from `render::format_duration`.
            let starts_with_verb = ["Replied", "Cooked", "Crunched", "Brewed"]
                .iter()
                .any(|v| s.starts_with(v));
            assert!(starts_with_verb, "unexpected verb in: {s}");
        }
        other => panic!("expected Note, got {other:?}"),
    }
}

#[test]
fn turn_end_without_user_does_not_emit_footer() {
    // A bare TurnEnd with no preceding push_user (transport error
    // before the turn really started, etc.) shouldn't emit a
    // duration note from a stale timer.
    let mut t = Timeline::new();
    t.apply(AgentEvent::TurnEnd {
        reason: FinishReason::Stop,
    });
    assert!(t.items().is_empty());
}

#[test]
fn turn_verb_keyed_by_duration() {
    assert_eq!(turn_verb(Duration::from_millis(500)), "Replied");
    assert_eq!(turn_verb(Duration::from_secs(2)), "Replied");
    assert_eq!(turn_verb(Duration::from_secs(3)), "Cooked");
    assert_eq!(turn_verb(Duration::from_secs(14)), "Cooked");
    assert_eq!(turn_verb(Duration::from_secs(15)), "Crunched");
    assert_eq!(turn_verb(Duration::from_secs(59)), "Crunched");
    assert_eq!(turn_verb(Duration::from_secs(60)), "Brewed");
    assert_eq!(turn_verb(Duration::from_secs(600)), "Brewed");
}

#[test]
fn clear_drops_all_items() {
    let mut t = Timeline::new();
    t.push_user("hi".into());
    t.push_assistant_text("hello".into());
    t.clear();
    assert!(t.is_empty());
}
