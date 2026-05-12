use super::*;
use crate::tui::test_support::test_app;
use crate::tui::timeline::TimelineItem;
use lumen_core::provider::FinishReason;

#[test]
fn assistant_text_event_does_not_flip_mode() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::AssistantText("hi".into())));
    assert_eq!(app.timeline.items().len(), 1);
    assert_eq!(app.mode, AppMode::Streaming);
}

#[test]
fn turn_end_event_flips_mode_to_idle_and_drops_handle() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    // turn_handle was already None at construction; verify
    // apply_ui_msg keeps it that way and flips mode. The
    // populated-handle case is exercised end-to-end through the
    // input::tests::enter_submits... async test (which spawns a
    // real task whose abort/clear path lives in input::cancel_turn).
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::TurnEnd {
        reason: FinishReason::Stop,
    }));
    assert_eq!(app.mode, AppMode::Idle);
    assert!(app.turn_handle.is_none());
}

#[test]
fn note_pushes_note_and_flips_idle() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    app.apply_ui_msg(UiMsg::Note("transport error".into()));
    assert_eq!(app.timeline.items().len(), 1);
    assert!(matches!(
        &app.timeline.items()[0],
        TimelineItem::Note(s) if s == "transport error"
    ));
    assert_eq!(app.mode, AppMode::Idle);
    assert!(app.turn_handle.is_none());
}

#[test]
fn input_is_empty_detects_whitespace_only() {
    let mut app = test_app();
    assert!(app.input_is_empty());
    app.set_input("   ");
    assert!(app.input_is_empty());
    app.set_input("hi");
    assert!(!app.input_is_empty());
}

#[test]
fn push_history_caps_at_capacity() {
    let mut app = test_app();
    for i in 0..(HISTORY_CAPACITY + 5) {
        app.history.push(format!("msg-{i}"));
    }
    assert_eq!(app.history.entries.len(), HISTORY_CAPACITY);
    // Oldest five dropped; newest preserved.
    assert_eq!(app.history.entries.front().unwrap(), "msg-5");
    assert_eq!(
        app.history.entries.back().unwrap(),
        &format!("msg-{}", HISTORY_CAPACITY + 4)
    );
}

#[test]
fn push_history_dedupes_consecutive_repeats() {
    let mut app = test_app();
    app.history.push("hello".into());
    app.history.push("hello".into());
    app.history.push("hello".into());
    assert_eq!(app.history.entries.len(), 1);
}

#[test]
fn push_history_dedupes_non_consecutive_repeats() {
    // Mode B (erasedups): a later occurrence replaces the earlier one.
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());
    app.history.push("first".into());
    assert_eq!(app.history.entries.len(), 2);
    // "first" moves to the back (most recent).
    assert_eq!(app.history.entries.front().unwrap(), "second");
    assert_eq!(app.history.entries.back().unwrap(), "first");
}

#[test]
fn push_history_resets_cursor_and_draft() {
    let mut app = test_app();
    app.history.entries.push_back("old".into());
    app.history.cursor = Some(0);
    app.history.draft = Some("draft".into());
    app.history.push("new".into());
    assert!(app.history.cursor.is_none());
    assert!(app.history.draft.is_none());
}

#[test]
fn set_input_replaces_buffer() {
    let mut app = test_app();
    app.set_input("multi\nline");
    assert_eq!(app.input.lines(), &["multi".to_string(), "line".to_string()]);
}

#[tokio::test]
async fn load_input_history_returns_empty_when_file_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    let loaded = load_input_history(&path).await;
    assert!(loaded.is_empty());
}

/// Build a history file from raw entries (mimicking what an older
/// append-only writer or a fresh save would produce).
async fn write_history_file(path: &Path, entries: &[&str]) {
    let snapshot: VecDeque<String> = entries.iter().map(|s| (*s).to_string()).collect();
    save_full_history(path, &snapshot).await.unwrap();
}

#[tokio::test]
async fn save_then_load_round_trips_entries_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    write_history_file(&path, &["first", "second", "third"]).await;

    let loaded = load_input_history(&path).await;
    let v: Vec<&str> = loaded.iter().map(String::as_str).collect();
    assert_eq!(v, vec!["first", "second", "third"]);
}

#[tokio::test]
async fn save_preserves_multiline_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    write_history_file(&path, &["line one\nline two", "after"]).await;

    let loaded = load_input_history(&path).await;
    let v: Vec<&str> = loaded.iter().map(String::as_str).collect();
    assert_eq!(v, vec!["line one\nline two", "after"]);
}

#[tokio::test]
async fn load_dedupes_keeping_last_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    // An old append-only file might have collected duplicates.
    write_history_file(&path, &["a", "b", "a", "c", "b"]).await;

    let loaded = load_input_history(&path).await;
    let v: Vec<&str> = loaded.iter().map(String::as_str).collect();
    // "a" -> kept (last at index 2), "b" -> kept (last at index 4),
    // "c" -> kept (only). Order preserved by last-occurrence position.
    assert_eq!(v, vec!["a", "c", "b"]);

    // File on disk also reflects the dedup.
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(raw.lines().count(), 3);
}

#[tokio::test]
async fn load_skips_invalid_lines_silently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    // Mix valid JSON-encoded entries with garbage.
    let raw = "\"valid\"\nnot-json\n\"also valid\"\n{broken\n\"third\"\n";
    tokio::fs::write(&path, raw).await.unwrap();

    let loaded = load_input_history(&path).await;
    let v: Vec<&str> = loaded.iter().map(String::as_str).collect();
    assert_eq!(v, vec!["valid", "also valid", "third"]);
}

#[tokio::test]
async fn load_cleans_up_orphan_tmp_file() {
    // A crash between `write(&tmp, ...)` and `rename(&tmp, path)`
    // leaves an `input_history.tmp` sibling behind. Load must
    // remove it so the data dir doesn't accumulate torn writes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, "garbage").await.unwrap();
    assert!(tmp.exists());

    let loaded = load_input_history(&path).await;
    assert!(loaded.is_empty(), "no canonical file -> empty result");
    assert!(!tmp.exists(), "load must self-heal the orphan tmp file");
}

#[tokio::test]
async fn rapid_pushes_persist_final_state_via_saver_task() {
    // Regression: the prior `tokio::spawn`-per-push pattern paired
    // each save with its own captured snapshot. Under a multi-thread
    // runtime, two spawned saves could acquire the write lock in
    // reverse order from the pushes, so an older snapshot would
    // `rename` over a newer one and silently lose entries.
    //
    // The single saver task funnels every write through one
    // FIFO channel and coalesces queued snapshots before each
    // `save_full_history` call - the most recent snapshot always
    // wins by construction. This test stresses that path.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");
    let (tx, handle) = spawn_history_saver(path.clone());

    let mut h = HistoryState::new();
    h.saver = Some(tx);

    let total = 200; // well under HISTORY_CAPACITY so nothing is evicted
    for i in 0..total {
        h.push(format!("e{i}"));
    }
    // Drop the sender so the saver task sees the channel close
    // after draining its queue, finishes its last write, and exits.
    drop(h);
    handle.await.unwrap();

    let on_disk = load_input_history(&path).await;
    let actual: Vec<&str> = on_disk.iter().map(String::as_str).collect();
    let expected_owned: Vec<String> = (0..total).map(|i| format!("e{i}")).collect();
    let expected: Vec<&str> = expected_owned.iter().map(String::as_str).collect();
    assert_eq!(actual, expected, "final on-disk state must match in-memory");
}

#[tokio::test]
async fn load_trims_oversized_file_and_rewrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input_history");

    // Write HISTORY_CAPACITY + 5 entries directly to the file.
    let extra = 5;
    let total = HISTORY_CAPACITY + extra;
    let mut content = String::new();
    for i in 0..total {
        content.push_str(&serde_json::to_string(&format!("e{i}")).unwrap());
        content.push('\n');
    }
    tokio::fs::write(&path, content).await.unwrap();

    // Load: should trim and rewrite.
    let loaded = load_input_history(&path).await;
    assert_eq!(loaded.len(), HISTORY_CAPACITY);
    // Newest entries kept, oldest dropped.
    assert_eq!(loaded.front().unwrap(), &format!("e{extra}"));
    assert_eq!(loaded.back().unwrap(), &format!("e{}", total - 1));

    // File on disk also reflects the cap now.
    let after_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(after_disk.lines().count(), HISTORY_CAPACITY);
}
