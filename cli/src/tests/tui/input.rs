use super::*;
use crate::tui::app::ApprovalKind;
use crate::tui::test_support::test_app;
use crate::tui::timeline::TimelineItem;

fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

fn type_str(app: &mut AppState, s: &str) {
    for c in s.chars() {
        handle_key(k(KeyCode::Char(c), KeyModifiers::NONE), app);
    }
}

// --- routing & textarea pass-through --------------------------- //

#[test]
fn plain_char_lands_in_input() {
    let mut app = test_app();
    type_str(&mut app, "hi");
    assert_eq!(app.input.lines(), &["hi".to_string()]);
}

#[test]
fn backspace_deletes_char() {
    let mut app = test_app();
    type_str(&mut app, "ab");
    handle_key(k(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["a".to_string()]);
}

#[test]
fn ctrl_left_word_jump_routes_through_textarea_default() {
    // Doc claim in the help overlay: "Ctrl+Left / Right" =
    // word jump. We don't bind this chord explicitly; the
    // catch-all `_ => app.input.input(k)` forwards it and
    // tui-textarea's default Input mapping handles word-back.
    // Pin the behavior so a refactor of the catch-all doesn't
    // silently break the documented binding.
    let mut app = test_app();
    type_str(&mut app, "hello world");
    assert_eq!(app.input.cursor(), (0, 11));
    handle_key(k(KeyCode::Left, KeyModifiers::CONTROL), &mut app);
    let (row, col) = app.input.cursor();
    assert_eq!(row, 0);
    assert!(col < 11, "Ctrl+Left should move cursor left, got col {col}");
    // Word-boundary policy puts us at or before the start of
    // "world" (col 6) - the exact column is tui-textarea's
    // call, but we must NOT land back at col 11.
    assert!(col <= 6, "Ctrl+Left should jump past 'world', got col {col}");
}

#[test]
fn ctrl_right_word_jump_routes_through_textarea_default() {
    let mut app = test_app();
    type_str(&mut app, "hello world");
    app.input.move_cursor(CursorMove::Head);
    assert_eq!(app.input.cursor(), (0, 0));
    handle_key(k(KeyCode::Right, KeyModifiers::CONTROL), &mut app);
    let (row, col) = app.input.cursor();
    assert_eq!(row, 0);
    assert!(col > 0, "Ctrl+Right should move cursor right, got col {col}");
}

#[test]
fn ctrl_backspace_deletes_word_back() {
    let mut app = test_app();
    type_str(&mut app, "hello world");
    handle_key(k(KeyCode::Backspace, KeyModifiers::CONTROL), &mut app);
    let after = app.input.lines().join("\n");
    // Loose assertion: exact behavior depends on tui-textarea's
    // word-boundary policy (whether the trailing space is consumed).
    // Important invariants: "world" is gone, "hello" survives,
    // result is shorter than the original.
    assert!(
        after.starts_with("hello"),
        "expected 'hello' prefix, got {after:?}"
    );
    assert!(!after.contains("world"));
    assert!(after.len() < "hello world".len());
}

#[test]
fn shift_enter_inserts_newline_in_input() {
    let mut app = test_app();
    type_str(&mut app, "a");
    handle_key(k(KeyCode::Enter, KeyModifiers::SHIFT), &mut app);
    type_str(&mut app, "b");
    assert_eq!(app.input.lines(), &["a".to_string(), "b".to_string()]);
    assert!(app.timeline.items().is_empty());
}

// --- Esc / Ctrl+C / Ctrl+D semantics --------------------------- //

#[test]
fn ctrl_d_always_quits() {
    let mut app = test_app();
    // Even with content typed, Ctrl+D quits.
    type_str(&mut app, "hello");
    assert_eq!(
        handle_key(k(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut app),
        Action::Quit
    );
}

#[test]
fn esc_double_tap_clears_non_empty_input() {
    use crate::tui::app::ArmedKey;
    let mut app = test_app();
    type_str(&mut app, "hello");
    // First Esc arms - buffer is untouched.
    let first = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(first, Action::Continue);
    assert!(!app.input_is_empty(), "first Esc must not clear");
    assert_eq!(app.armed_key(), Some(ArmedKey::Esc));
    // Second Esc clears + disarms.
    let second = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(second, Action::Continue);
    assert!(app.input_is_empty());
    assert!(app.armed_key().is_none(), "clear should disarm");
}


#[test]
fn alt_enter_inserts_newline_not_submit() {
    let mut app = test_app();
    type_str(&mut app, "a");
    handle_key(k(KeyCode::Enter, KeyModifiers::ALT), &mut app);
    type_str(&mut app, "b");
    assert_eq!(
        app.input.lines(),
        &["a".to_string(), "b".to_string()],
        "Alt+Enter should insert a newline"
    );
    assert!(
        app.timeline.items().is_empty(),
        "Alt+Enter should not submit"
    );
}

#[test]
fn ctrl_c_clears_non_empty_input() {
    let mut app = test_app();
    type_str(&mut app, "hello");
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(app.input_is_empty());
}

#[test]
fn ctrl_c_arms_first_then_quits_second_on_idle_empty() {
    use crate::tui::app::ArmedKey;
    let mut app = test_app();
    let first = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(first, Action::Continue);
    assert_eq!(app.armed_key(), Some(ArmedKey::CtrlC));
    let second = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(second, Action::Quit);
}

#[test]
fn esc_arms_first_then_quits_second_on_idle_empty() {
    use crate::tui::app::ArmedKey;
    let mut app = test_app();
    let first = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(first, Action::Continue);
    assert_eq!(app.armed_key(), Some(ArmedKey::Esc));
    let second = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(second, Action::Quit);
}

#[test]
fn any_key_between_esc_presses_disarms() {
    let mut app = test_app();
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert!(app.armed_key().is_some());
    // Type a character -> resets armed state and now input has content.
    type_str(&mut app, "x");
    assert!(app.armed_key().is_none());
    // Non-empty input -> next Esc just arms again (double-tap to clear).
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(!app.input_is_empty(), "single Esc must not clear input");
    assert!(app.armed_key().is_some(), "first Esc on content arms");
}

#[test]
fn cross_chord_key_disarms() {
    // Pressing Ctrl+C while armed-with-Esc must NOT confirm Esc-quit.
    use crate::tui::app::ArmedKey;
    let mut app = test_app();
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(app.armed_key(), Some(ArmedKey::Esc));
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    // Ctrl+C on idle+empty (now disarmed from Esc) starts its own
    // arm cycle - it must not quit.
    assert_eq!(action, Action::Continue);
    assert_eq!(app.armed_key(), Some(ArmedKey::CtrlC));
}

#[test]
fn arm_state_expires_after_timeout() {
    use crate::tui::app::{ARM_TIMEOUT, ArmState, ArmedKey};
    use std::time::Instant;
    let mut app = test_app();
    // Hand-roll an arm-state stamped just past the timeout boundary
    // so we don't have to actually sleep in the test.
    app.arm_state = Some(ArmState {
        key: ArmedKey::Esc,
        at: Instant::now()
            .checked_sub(ARM_TIMEOUT + std::time::Duration::from_millis(1))
            .expect("Instant should be past the epoch"),
    });
    assert!(app.armed_key().is_none(), "expired arm reads as disarmed");
    // The next Esc should arm fresh (not confirm-quit).
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert_eq!(app.armed_key(), Some(ArmedKey::Esc));
}

#[test]
fn esc_during_streaming_cancels_and_keeps_running() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert_eq!(app.mode, AppMode::Idle);
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s == "cancelled by user"
    ));
    assert!(app.armed_key().is_none(), "cancel shouldn't arm");
}

#[test]
fn ctrl_c_during_streaming_cancels_and_keeps_running() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Continue);
    assert_eq!(app.mode, AppMode::Idle);
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s == "cancelled by user"
    ));
}

#[test]
fn esc_during_streaming_with_input_cancels_turn_and_preserves_input() {
    // Streaming takes priority: the in-flight turn is the expensive
    // thing, one press stops it. The user's typed draft survives
    // the cancel so they can edit and resubmit.
    let mut app = test_app();
    type_str(&mut app, "draft text");
    app.mode = AppMode::Streaming;
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert_eq!(app.mode, AppMode::Idle);
    // Draft preserved.
    assert_eq!(app.input.lines(), &["draft text".to_string()]);
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s == "cancelled by user"
    ));
}

// --- submit + history ------------------------------------------ //

#[test]
fn check_and_take_input_blocks_when_streaming() {
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    type_str(&mut app, "wait");
    let taken = check_and_take_input(&mut app);
    assert!(taken.is_none());
    assert_eq!(app.input.lines(), &["wait".to_string()]);
}

#[test]
fn check_and_take_input_returns_text_on_idle_with_content() {
    let mut app = test_app();
    type_str(&mut app, "hello");
    let taken = check_and_take_input(&mut app);
    assert_eq!(taken.as_deref(), Some("hello"));
    assert_eq!(app.mode, AppMode::Streaming);
    assert!(app.input_is_empty());
    assert_eq!(app.timeline.items().len(), 1);
    assert!(matches!(
        &app.timeline.items()[0],
        TimelineItem::User(s) if s == "hello"
    ));
}

#[test]
fn empty_enter_does_nothing() {
    let mut app = test_app();
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(app.timeline.items().is_empty());
    assert_eq!(app.mode, AppMode::Idle);
}

#[test]
fn up_arrow_recalls_most_recent_history() {
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["second".to_string()]);
    assert_eq!(app.history.cursor, Some(1));
}

#[test]
fn up_arrow_walks_back_through_history() {
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["first".to_string()]);
    assert_eq!(app.history.cursor, Some(0));
}

#[test]
fn up_arrow_at_oldest_stays_put() {
    let mut app = test_app();
    app.history.push("only".into());
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["only".to_string()]);
    assert_eq!(app.history.cursor, Some(0));
}

#[test]
fn down_past_newest_clears_input() {
    // With strict "empty-only" recall, the draft-snapshot branch
    // can never trigger (you can't start recall with input typed),
    // so Down past newest just clears.
    let mut app = test_app();
    app.history.push("old".into());
    // Start recall from empty input.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["old".to_string()]);
    // Down past newest -> no draft to restore -> empty input.
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert!(app.input_is_empty());
    assert!(app.history.cursor.is_none());
}

#[test]
fn up_in_multiline_navigates_to_top_then_to_col_0_then_recalls() {
    // The fish-style "nudge to edge first": Up moves cursor up
    // through the buffer, then to column 0 of the top row, and
    // only THEN recalls history.
    let mut app = test_app();
    app.history.push("history".into());
    type_str(&mut app, "line1");
    handle_key(k(KeyCode::Enter, KeyModifiers::SHIFT), &mut app);
    type_str(&mut app, "line2"); // cursor at (1, 5)

    // 1st Up: row 1 -> row 0, col stays at 5.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 5));
    assert!(app.history.cursor.is_none());

    // 2nd Up: row 0 col 5 -> not (0,0), tui-textarea Up moves to (0, 0).
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 0));
    assert!(app.history.cursor.is_none());

    // 3rd Up: at (0,0) -> recall.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["history".to_string()]);
    assert_eq!(app.history.cursor, Some(0));
}

#[test]
fn up_with_single_line_text_nudges_to_col_0_first() {
    // Single-line input "draft" with cursor at end (0, 5):
    //   - 1st Up: not at (0,0), so textarea moves cursor to (0, 0)
    //   - 2nd Up: at (0, 0), recall fires
    let mut app = test_app();
    app.history.push("old".into());
    type_str(&mut app, "draft");
    assert_eq!(app.input.cursor(), (0, 5));

    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 0));
    assert!(app.history.cursor.is_none());

    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["old".to_string()]);
    assert_eq!(app.history.cursor, Some(0));
}

#[test]
fn down_at_end_of_last_line_recalls_newer() {
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());
    // Browse back to "first".
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["first".to_string()]);
    // Down (already browsing) walks forward to "second".
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["second".to_string()]);
}

#[test]
fn editing_recalled_entry_detaches_from_history_mode() {
    // After recall + edit, Up/Down should behave as cursor
    // navigation + edge-nudge, NOT continued history rotation.
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());
    // Recall "second" - now in history-browse mode (cursor=Some(1)).
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["second".to_string()]);
    assert_eq!(app.history.cursor, Some(1));

    // Type a char to diverge from the recalled entry.
    type_str(&mut app, "!");
    assert_eq!(app.input.lines(), &["second!".to_string()]);
    // The divergence is detected on the NEXT keystroke (the
    // detach check runs at top of handle_key). The cursor's
    // state right after the typing event itself is still in
    // browse-mode; the next event clears it.
    handle_key(k(KeyCode::Left, KeyModifiers::NONE), &mut app);
    assert!(
        app.history.cursor.is_none(),
        "history-browse mode should have detached after the edit"
    );
}

#[test]
fn up_after_editing_recalled_entry_nudges_instead_of_recalling() {
    // The user's reported scenario: recall something, edit it,
    // press Up. Should nudge the cursor (or pass through to
    // textarea) rather than walking to an older entry.
    let mut app = test_app();
    app.history.push("first".into());
    app.history.push("second".into());

    // Recall "second" and modify it to "secondX".
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    type_str(&mut app, "X");
    // Cursor is at end of "secondX" -> (0, 7).
    // The detach check fires on this next Up press.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    // We should NOT have rotated to "first". Input should
    // still be the edited version (with possibly a cursor
    // nudge applied).
    let lines = app.input.lines().join("\n");
    assert!(
        lines.contains("secondX"),
        "edited input should be preserved, not replaced by older history; got: {lines}"
    );
    assert!(
        !lines.starts_with("first"),
        "should not have rotated to older history entry"
    );
}

#[test]
fn pure_cursor_movement_keeps_history_browse_mode() {
    // Left / Right / Home / End don't modify the buffer.
    // History-browse mode should survive them so the user
    // can navigate within the recalled entry, then press
    // Up to go further back.
    let mut app = test_app();
    app.history.push("hello world".into());
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.history.cursor, Some(0));
    // Move cursor without modifying. History mode should
    // remain active.
    handle_key(k(KeyCode::Left, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Left, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.history.cursor,
        Some(0),
        "cursor-only movement must not detach from history mode"
    );
}

// --- wrap-aware Up/Down navigation ----------------------------- //

/// Long pasted line: 25 chars at width 10 wraps to 3 visual rows
/// (0..10, 10..20, 20..25). Cursor at end -> visual (2, 5).
/// Up must land at visual (1, 5) = logical (0, 15), NOT at
/// (0, 0) like the old logical-only dispatcher did.
#[test]
fn wrap_aware_up_moves_within_wrapped_line_preserving_vcol() {
    let mut app = test_app();
    app.render.last_textarea_width = 10;
    type_str(&mut app, "0123456789ABCDEFGHIJKLMNO"); // 25 chars
    assert_eq!(app.input.cursor(), (0, 25));

    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.input.cursor(),
        (0, 15),
        "Up on wrap-continuation row must move one visual row up, not nudge to head"
    );
    assert!(
        app.history.cursor.is_none(),
        "Up inside wrap must not engage history"
    );
}

/// Continuing the above: another Up from (0, 15) -> (0, 5).
/// A third Up from (0, 5) is at the first visual subrow with
/// vcol > 0 -> nudge to head. A fourth Up at (0, 0) -> recall.
#[test]
fn wrap_aware_up_walks_visual_rows_then_nudges_then_recalls() {
    let mut app = test_app();
    app.render.last_textarea_width = 10;
    app.history.push("old".into());
    type_str(&mut app, "0123456789ABCDEFGHIJKLMNO"); // 25 chars
    assert_eq!(app.input.cursor(), (0, 25));

    // (2, 5) -> (1, 5)
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 15));
    // (1, 5) -> (0, 5)
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 5));
    // (0, 5) -> (0, 0) [nudge]
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 0));
    // (0, 0) -> recall older history.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.lines(), &["old".to_string()]);
}

/// Down mirror: from logical (0, 0), Down walks down through
/// visual rows within the wrapped logical line.
#[test]
fn wrap_aware_down_walks_visual_rows_within_wrapped_line() {
    let mut app = test_app();
    app.render.last_textarea_width = 10;
    type_str(&mut app, "0123456789ABCDEFGHIJKLMNO"); // 25 chars
    // Move to start of buffer.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app); // (0,25) -> (0,15)
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app); // -> (0,5)
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app); // -> (0,0)
    assert_eq!(app.input.cursor(), (0, 0));

    // (0, 0) -> (1, 0) visually = logical (0, 10).
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 10));
    // -> logical (0, 20)
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 20));
}

/// Down at end of buffer recalls newer (history-browse) or
/// stays put (no history). With history loaded but not
/// currently browsing, Down at end is a no-op (matches the
/// existing `down_past_newest_clears_input` shape: you have
/// to be IN browse mode for Down to walk history).
#[test]
fn wrap_aware_down_at_visual_bottom_does_not_eat_into_history() {
    let mut app = test_app();
    app.render.last_textarea_width = 10;
    app.history.push("prior".into());
    type_str(&mut app, "0123456789ABCDE"); // 15 chars, wraps to 2 visual rows
    assert_eq!(app.input.cursor(), (0, 15)); // end of buffer

    // At visual bottom + end-of-buffer with no active history
    // browse -> recall_newer is called but it returns early
    // (cursor.is_none()), so nothing happens.
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor(), (0, 15));
    assert_eq!(
        app.input.lines(),
        &["0123456789ABCDE".to_string()],
        "Down at end must not replace input"
    );
}

/// Cross-logical-line Up: cursor on the second logical line
/// must move to the LAST visual subrow of the previous
/// logical line (not its first), preserving visual column.
#[test]
fn wrap_aware_up_crosses_logical_line_to_last_visual_subrow() {
    let mut app = test_app();
    app.render.last_textarea_width = 10;
    // Line 0: 25 chars (3 visual subrows: 0..10, 10..20, 20..25).
    type_str(&mut app, "0123456789ABCDEFGHIJKLMNO");
    // Newline -> line 1.
    handle_key(k(KeyCode::Enter, KeyModifiers::SHIFT), &mut app);
    // Line 1: 5 chars.
    type_str(&mut app, "XY-AB");
    assert_eq!(app.input.cursor(), (1, 5));

    // Up from logical (1, 5) -> last visual subrow of line 0
    // (subrow 2 = chars 20..25), preserving vcol 5 -> clamps
    // to line end col 25.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.input.cursor(),
        (0, 25),
        "Up across logical lines must target the LAST visual subrow above"
    );
}

#[test]
fn down_in_middle_of_multiline_moves_cursor_not_history() {
    let mut app = test_app();
    app.history.push("history".into());
    type_str(&mut app, "line1");
    handle_key(k(KeyCode::Enter, KeyModifiers::SHIFT), &mut app);
    type_str(&mut app, "line2");
    // Move cursor up to row 0.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor().0, 0);
    // Down at row 0 (not the last row) -> cursor goes to row 1.
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.input.cursor().0, 1);
    assert!(app.history.cursor.is_none());
}

// --- mouse selection + auto-copy config ------------------------ //

fn drag_select(app: &mut AppState, from: (u16, u16), to: (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: from.0,
            row: from.1,
            modifiers: KeyModifiers::NONE,
        },
        app,
    );
    handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: to.0,
            row: to.1,
            modifiers: KeyModifiers::NONE,
        },
        app,
    );
    handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: to.0,
            row: to.1,
            modifiers: KeyModifiers::NONE,
        },
        app,
    );
}

#[test]
fn drag_release_sets_copy_pending_by_default() {
    let mut app = test_app();
    // Default config: auto_copy_on_select = true.
    assert!(app.cfg.ui.auto_copy_on_select);
    drag_select(&mut app, (5, 5), (10, 5));
    assert!(app.selection.is_some(), "selection should persist after release");
    assert!(
        app.render.copy_pending,
        "default config should auto-copy on release"
    );
}

#[test]
fn drag_release_leaves_copy_pending_off_when_auto_copy_disabled() {
    let mut app = test_app();
    app.cfg.ui.auto_copy_on_select = false;
    drag_select(&mut app, (5, 5), (10, 5));
    assert!(app.selection.is_some());
    assert!(
        !app.render.copy_pending,
        "auto_copy_on_select=false should leave copy_pending unset"
    );
}

#[test]
fn click_without_drag_clears_selection_regardless_of_config() {
    let mut app = test_app();
    app.cfg.ui.auto_copy_on_select = true;
    // Down + Up at the same cell with no Drag = click without drag.
    drag_select(&mut app, (5, 5), (5, 5));
    assert!(app.selection.is_none());
    assert!(!app.render.copy_pending);
}

// --- async submit through full spawn path ---------------------- //

// --- Shift+Tab approval-mode toggle ---------------------------- //

#[test]
fn shift_tab_toggles_approval_mode_between_never_and_safe() {
    use lumen_core::AutoApply;
    let mut app = test_app();
    assert_eq!(app.auto_apply(), AutoApply::Never);
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    assert_eq!(app.auto_apply(), AutoApply::Safe);
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    assert_eq!(app.auto_apply(), AutoApply::Never);
}

#[test]
fn kitty_protocol_shift_tab_also_toggles() {
    // On terminals with the Kitty Keyboard Protocol pushed
    // (setup_terminal does this), Shift+Tab arrives as
    // `KeyCode::Tab` with `SHIFT` rather than `KeyCode::BackTab`.
    use lumen_core::AutoApply;
    let mut app = test_app();
    handle_key(k(KeyCode::Tab, KeyModifiers::SHIFT), &mut app);
    assert_eq!(app.auto_apply(), AutoApply::Safe);
}

#[test]
fn cycling_does_not_emit_a_timeline_note() {
    // Visible confirmation is the policy-hint row's label
    // changing color; pushing a note for every flip would
    // pollute the conversation if the user spammed Shift+Tab.
    let mut app = test_app();
    let before = app.timeline.items().len();
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    assert_eq!(
        app.timeline.items().len(),
        before,
        "Shift+Tab must not push timeline items"
    );
}

#[test]
fn modal_swallows_shift_tab_so_toggle_doesnt_fire_mid_approval() {
    // Inside the approval modal, every key except y/n/Esc is
    // swallowed. Shift+Tab during a prompt must NOT cycle the
    // policy (would auto-resolve the pending prompt and surprise
    // the user). They have to answer y/n first.
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    let initial = app.auto_apply();
    handle_key(k(KeyCode::BackTab, KeyModifiers::NONE), &mut app);
    assert_eq!(app.auto_apply(), initial, "modal must swallow Shift+Tab");
    assert!(app.pending_approval.is_some(), "modal must stay up");
}

// --- approval modal intercept ---------------------------------- //

fn pending_diff(app: &mut AppState) -> tokio::sync::oneshot::Receiver<Verdict> {
    use super::super::app::PendingApproval;
    use std::path::PathBuf;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Diff {
            path: PathBuf::from("f.rs"),
            diff: "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
        },
        reply: tx,
        selected: 0,
    });
    rx
}

fn pending_shell(app: &mut AppState) -> tokio::sync::oneshot::Receiver<Verdict> {
    use super::super::app::PendingApproval;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Shell {
            command: "rm -rf /".into(),
        },
        reply: tx,
        selected: 0,
    });
    rx
}

#[test]
fn pending_approval_y_sends_accept_and_clears() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Char('y'), KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none(), "modal should dismiss");
    assert_eq!(rx.try_recv().unwrap(), Verdict::Accept);
}

#[test]
fn pending_approval_uppercase_y_also_accepts() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Char('Y'), KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.try_recv().unwrap(), Verdict::Accept);
}

#[test]
fn pending_approval_n_sends_reject_and_clears() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Char('n'), KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.try_recv().unwrap(), Verdict::Reject);
}

#[test]
fn pending_approval_esc_sends_reject_and_clears() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.try_recv().unwrap(), Verdict::Reject);
}

// --- approval menu: arrow nav + multi-option ------------------- //

#[test]
fn down_arrow_advances_selection_capped_at_max() {
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 0);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 1);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 2);
    // Past the last option: clamp, don't wrap.
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 2);
}

#[test]
fn up_arrow_walks_back_clamped_at_zero() {
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    // Advance, then walk back.
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 2);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 1);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 0);
    // Past the first: clamp.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.pending_approval.as_ref().unwrap().selected, 0);
}

#[test]
fn enter_confirms_selected_accept() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    // Default selected = 0 (Accept).
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.try_recv().unwrap(), Verdict::Accept);
}

#[test]
fn enter_confirms_selected_reject_when_navigated() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    // Now at index 2 = Reject.
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(app.pending_approval.is_none());
    assert_eq!(rx.try_recv().unwrap(), Verdict::Reject);
}

#[test]
fn a_shortcut_accepts_all_and_flips_diff_mode_to_safe() {
    let mut app = test_app();
    let mut rx = pending_diff(&mut app);
    assert_eq!(app.auto_apply(), AutoApply::Never);
    handle_key(k(KeyCode::Char('a'), KeyModifiers::NONE), &mut app);
    // Sent Accept...
    assert_eq!(rx.try_recv().unwrap(), Verdict::Accept);
    // ...and flipped mode to Safe (auto-edits, prompt dangerous shell).
    assert_eq!(app.auto_apply(), AutoApply::Safe);
    assert!(app.pending_approval.is_none());
}

#[test]
fn a_shortcut_on_shell_prompt_is_no_op() {
    // Shell has no "Accept all" until per-command allowlisting
    // lands. The `a` shortcut must do nothing rather than
    // silently mapping to a different option.
    let mut app = test_app();
    let mut rx = pending_shell(&mut app);
    assert_eq!(app.auto_apply(), AutoApply::Never);
    handle_key(k(KeyCode::Char('a'), KeyModifiers::NONE), &mut app);
    // Prompt still up, no verdict sent, policy unchanged.
    assert!(app.pending_approval.is_some(), "modal should remain up");
    assert!(matches!(
        rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(app.auto_apply(), AutoApply::Never);
}

#[test]
fn accept_all_does_not_emit_timeline_note() {
    // Visible confirmation lives in the policy-hint row's color
    // flip - no inline note that would clutter the conversation.
    let mut app = test_app();
    let len_before = app.timeline.items().len();
    let _rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Char('a'), KeyModifiers::NONE), &mut app);
    // No new timeline items pushed by the policy flip.
    assert_eq!(app.timeline.items().len(), len_before);
    // ... but the policy actually changed.
    assert_eq!(app.auto_apply(), AutoApply::Safe);
}

#[test]
fn pending_approval_swallows_other_keys() {
    // Typing into the input mid-modal would be surprising on
    // dismissal. Any non-y/n/Esc key is dropped.
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    type_str(&mut app, "hello");
    assert!(app.pending_approval.is_some(), "modal stays up");
    assert!(app.input.lines().iter().all(String::is_empty),
        "input must not receive keystrokes while modal is up");
}

#[test]
fn ctrl_d_quits_even_when_approval_pending() {
    // Universal escape hatch must pierce the approval modal.
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    let action = handle_key(k(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Quit);
}

#[test]
fn ctrl_c_cancels_turn_when_approval_pending() {
    // Ctrl+C during approval cancels the whole turn; the
    // pending approval clears via `cancel_turn`'s
    // `pending_approval = None`. The verdict-receiver
    // (held by the tool task) will see RecvError on the
    // dropped sender and resolve to `Verdict::Reject`,
    // but the agent task is being aborted anyway.
    use crate::tui::timeline::TimelineItem;
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    let _rx = pending_diff(&mut app);
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(app.pending_approval.is_none(), "approval should clear");
    assert_eq!(app.mode, AppMode::Idle, "turn should be cancelled");
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s == "cancelled by user"
    ));
}

#[test]
fn pending_approval_does_not_arm_esc_quit() {
    // Esc-in-modal is a Reject, not the start of the
    // "press Esc twice to quit" gesture.
    let mut app = test_app();
    let _rx = pending_diff(&mut app);
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert!(app.armed_key().is_none(), "modal Esc must not arm quit");
}

#[test]
fn help_overlay_esc_closes() {
    let mut app = test_app();
    app.show_help = true;
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(!app.show_help);
}

#[test]
fn help_overlay_ctrl_c_closes() {
    let mut app = test_app();
    app.show_help = true;
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(!app.show_help, "Ctrl+C should close help overlay");
}

#[test]
fn help_overlay_ctrl_d_quits_through_modal() {
    // Ctrl+D bypasses the modal entirely - the user is never
    // trapped behind an overlay.
    let mut app = test_app();
    app.show_help = true;
    let action = handle_key(k(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Quit);
}

#[test]
fn help_overlay_swallows_other_keys() {
    let mut app = test_app();
    app.show_help = true;
    handle_key(k(KeyCode::Char('a'), KeyModifiers::NONE), &mut app);
    assert!(app.show_help, "non-dismiss keys must not close the overlay");
    assert!(
        app.input_is_empty(),
        "keystrokes behind the overlay must not reach the input"
    );
}

#[test]
fn question_mark_inserts_literal_and_does_not_open_help() {
    // `?` was previously a chord to open help. Now /help is the
    // only way; `?` is just a literal char regardless of where
    // the cursor is in the buffer.
    let mut app = test_app();
    handle_key(k(KeyCode::Char('?'), KeyModifiers::NONE), &mut app);
    assert!(!app.show_help);
    assert_eq!(app.input.lines(), &["?".to_string()]);
    type_str(&mut app, "what");
    handle_key(k(KeyCode::Char('?'), KeyModifiers::NONE), &mut app);
    assert!(!app.show_help);
    assert_eq!(app.input.lines(), &["?what?".to_string()]);
}

// --- slash palette ---------------------------------------------- //

#[test]
fn slash_on_empty_input_opens_palette() {
    let mut app = test_app();
    handle_key(k(KeyCode::Char('/'), KeyModifiers::NONE), &mut app);
    assert!(app.slash_palette.is_some());
    assert_eq!(app.input.lines(), &["/".to_string()]);
}

#[test]
fn slash_mid_message_inserts_literal_not_opens() {
    let mut app = test_app();
    type_str(&mut app, "hello");
    handle_key(k(KeyCode::Char('/'), KeyModifiers::NONE), &mut app);
    assert!(app.slash_palette.is_none());
    assert_eq!(app.input.lines(), &["hello/".to_string()]);
}

#[test]
fn typing_after_slash_filters_palette() {
    let mut app = test_app();
    type_str(&mut app, "/he");
    assert!(app.slash_palette.is_some());
    // Verify the filter narrowed: a typo continues to keep the
    // palette open but selected stays clamped at 0.
    type_str(&mut app, "lp");
    assert!(app.slash_palette.is_some());
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
}

#[test]
fn backspace_past_slash_closes_palette() {
    let mut app = test_app();
    type_str(&mut app, "/he");
    assert!(app.slash_palette.is_some());
    handle_key(k(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
    handle_key(k(KeyCode::Backspace, KeyModifiers::NONE), &mut app);
    // Input is now empty; palette should have closed via post-pass.
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
}

#[test]
fn esc_in_palette_closes_and_clears() {
    let mut app = test_app();
    type_str(&mut app, "/he");
    let action = handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
}

#[test]
fn ctrl_c_in_palette_closes_and_clears() {
    let mut app = test_app();
    type_str(&mut app, "/he");
    let action = handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
}

#[test]
fn ctrl_d_in_palette_quits_through() {
    let mut app = test_app();
    type_str(&mut app, "/h");
    let action = handle_key(k(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Quit);
}

#[test]
fn down_in_palette_moves_selection() {
    let mut app = test_app();
    handle_key(k(KeyCode::Char('/'), KeyModifiers::NONE), &mut app);
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 1);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
    // Up at top is clamped, no underflow.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
}

#[test]
fn down_in_palette_clamps_to_last_match() {
    let mut app = test_app();
    // `/h` filters to only "help" - Down should not move past 0.
    type_str(&mut app, "/h");
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.slash_palette.as_ref().unwrap().selected, 0);
}

#[test]
fn enter_runs_help_command() {
    let mut app = test_app();
    type_str(&mut app, "/help");
    let action = handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    assert!(app.show_help, "/help should open the help overlay");
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
}

#[test]
fn enter_runs_quit_command() {
    let mut app = test_app();
    type_str(&mut app, "/quit");
    let action = handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Quit);
}

#[tokio::test]
async fn enter_runs_bare_model_opens_picker_in_loading_state() {
    use crate::tui::model_picker::ModelPickerStatus;
    let mut app = test_app();
    type_str(&mut app, "/model");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
    let picker = app.model_picker.as_ref().expect("picker should be open");
    assert!(matches!(picker.status, ModelPickerStatus::Loading));
}

#[test]
fn enter_runs_model_with_inline_arg_switches_live() {
    let mut app = test_app();
    let prev_model = app.cfg.provider.model.clone();
    assert_ne!(prev_model, "gpt-4o", "test precondition");
    type_str(&mut app, "/model gpt-4o");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(app.cfg.provider.model, "gpt-4o");
    assert!(app.slash_palette.is_none());
    assert!(app.model_picker.is_none(), "inline arg path skips picker");
    assert!(app.input_is_empty());
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s.contains("switched to gpt-4o")
    ));
}

#[test]
fn enter_runs_model_with_inline_arg_persists_to_config_file() {
    // Wire a temp config path so /model writes through. Verifies
    // the integration between switch_model and
    // Config::set_model_in_file actually fires; the lower-level
    // file-content assertions live in the core tests.
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    app.cfg_path = Some(path.clone());
    type_str(&mut app, "/model gpt-4o-mini");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(path.exists(), "config file should be created");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("model = \"gpt-4o-mini\""));
    // No "in-memory only" warning note when persist succeeded.
    let notes_text: String = app
        .timeline
        .items()
        .iter()
        .filter_map(|it| match it {
            TimelineItem::Note(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !notes_text.contains("in-memory only"),
        "no fallback note when persist succeeds; got: {notes_text}"
    );
}

#[test]
fn enter_runs_settings_opens_overlay() {
    let mut app = test_app();
    type_str(&mut app, "/settings");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(app.settings.is_some(), "/settings should open the overlay");
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
}

#[test]
fn settings_overlay_esc_closes() {
    use crate::tui::settings::SettingsState;
    let mut app = test_app();
    app.settings = Some(SettingsState::new());
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    assert!(app.settings.is_none());
}

#[test]
fn settings_overlay_arrow_keys_navigate_selection() {
    use crate::tui::settings::{Field, SettingsState};
    let mut app = test_app();
    app.settings = Some(SettingsState::new());
    assert_eq!(app.settings.as_ref().unwrap().selected, 0);
    handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    assert_eq!(app.settings.as_ref().unwrap().selected, 1);
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.settings.as_ref().unwrap().selected, 0);
    // Up at top clamps.
    handle_key(k(KeyCode::Up, KeyModifiers::NONE), &mut app);
    assert_eq!(app.settings.as_ref().unwrap().selected, 0);
    // Down past last clamps.
    for _ in 0..Field::ALL.len() + 5 {
        handle_key(k(KeyCode::Down, KeyModifiers::NONE), &mut app);
    }
    assert_eq!(
        app.settings.as_ref().unwrap().selected,
        Field::ALL.len() - 1
    );
}

#[test]
fn settings_overlay_enter_on_bool_field_toggles() {
    use crate::tui::settings::{Field, SettingsState};
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    app.cfg_path = Some(dir.path().join("config.toml"));
    let initial = app.cfg.ui.auto_copy_on_select;
    // Select the auto_copy_on_select field.
    let idx = Field::ALL
        .iter()
        .position(|f| *f == Field::UiAutoCopyOnSelect)
        .unwrap();
    app.settings = Some(SettingsState {
        selected: idx,
        editing: None,
    });
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_ne!(
        app.cfg.ui.auto_copy_on_select, initial,
        "Enter on bool should toggle"
    );
    // Persisted to file.
    let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(content.contains("[ui]"));
    assert!(content.contains("auto_copy_on_select"));
}

#[test]
fn settings_overlay_enter_on_text_field_enters_edit_mode() {
    // `ProviderModel` is special-cased to open the picker; use
    // another Text field here to exercise the edit-buffer path.
    use crate::tui::settings::{Field, SettingsState};
    let mut app = test_app();
    let idx = Field::ALL
        .iter()
        .position(|f| *f == Field::ProviderBaseUrl)
        .unwrap();
    app.settings = Some(SettingsState {
        selected: idx,
        editing: None,
    });
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    let s = app.settings.as_ref().unwrap();
    assert!(s.editing.is_some(), "Enter on text should open edit buffer");
    // Buffer is pre-seeded with the current value.
    assert_eq!(
        s.editing.as_ref().unwrap().buffer,
        app.cfg.provider.base_url
    );
}

#[tokio::test]
async fn settings_overlay_enter_on_model_field_opens_picker() {
    use crate::tui::settings::{Field, SettingsState};
    let mut app = test_app();
    let idx = Field::ALL
        .iter()
        .position(|f| *f == Field::ProviderModel)
        .unwrap();
    app.settings = Some(SettingsState {
        selected: idx,
        editing: None,
    });
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(
        app.model_picker.is_some(),
        "Enter on provider.model should open the model picker"
    );
    // Settings overlay stays open behind the picker - user sees
    // updated value when picker commits.
    assert!(app.settings.is_some());
}

#[test]
fn settings_overlay_edit_commit_writes_to_cfg_and_file() {
    use crate::tui::settings::{EditBuffer, Field, SettingsState};
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    app.cfg_path = Some(dir.path().join("config.toml"));
    let idx = Field::ALL
        .iter()
        .position(|f| *f == Field::ProviderModel)
        .unwrap();
    app.settings = Some(SettingsState {
        selected: idx,
        editing: Some(EditBuffer {
            buffer: "new-cool-model".into(),
        }),
    });
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(app.cfg.provider.model, "new-cool-model");
    assert!(app.settings.as_ref().unwrap().editing.is_none(), "edit committed");
    let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(content.contains("model = \"new-cool-model\""));
}

#[test]
fn settings_overlay_edit_esc_cancels_without_committing() {
    use crate::tui::settings::{EditBuffer, Field, SettingsState};
    let mut app = test_app();
    let prev = app.cfg.provider.model.clone();
    let idx = Field::ALL
        .iter()
        .position(|f| *f == Field::ProviderModel)
        .unwrap();
    app.settings = Some(SettingsState {
        selected: idx,
        editing: Some(EditBuffer {
            buffer: "should-not-stick".into(),
        }),
    });
    handle_key(k(KeyCode::Esc, KeyModifiers::NONE), &mut app);
    // Cancelled - cfg unchanged, edit mode exited but modal stays.
    assert_eq!(app.cfg.provider.model, prev);
    assert!(app.settings.is_some());
    assert!(app.settings.as_ref().unwrap().editing.is_none());
}

#[test]
fn settings_overlay_ctrl_d_quits_through() {
    use crate::tui::settings::SettingsState;
    let mut app = test_app();
    app.settings = Some(SettingsState::new());
    let action = handle_key(k(KeyCode::Char('d'), KeyModifiers::CONTROL), &mut app);
    assert_eq!(action, Action::Quit);
}

#[test]
fn enter_runs_model_with_same_name_is_noop() {
    let mut app = test_app();
    // Default model is "" (unset sentinel - opens the picker when
    // typed as `/model ` with empty arg). Seed a real name so the
    // `/model <name>` path is exercised against itself.
    app.cfg.provider.model = "qwen2.5-coder".to_string();
    let current = app.cfg.provider.model.clone();
    type_str(&mut app, &format!("/model {current}"));
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(app.cfg.provider.model, current);
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s.contains("already set to")
    ));
}

#[test]
fn enter_runs_clear_command() {
    let mut app = test_app();
    // Seed the timeline so we can verify it gets wiped.
    app.timeline.push_user("prior message".into());
    assert!(!app.timeline.items().is_empty());
    type_str(&mut app, "/clear");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    // Only the "conversation cleared" note should remain.
    assert!(app.slash_palette.is_none());
    assert!(app.input_is_empty());
    assert_eq!(app.timeline.items().len(), 1);
    assert!(matches!(
        &app.timeline.items()[0],
        TimelineItem::Note(s) if s == "conversation cleared"
    ));
}

#[tokio::test]
async fn enter_runs_clear_falls_back_when_agent_locked() {
    let mut app = test_app();
    // Simulate a mid-flight turn: hold the agent lock so the
    // /clear handler's `try_lock` fails. The handler must emit
    // a fallback note instead of touching the timeline.
    let _guard = app.agent.clone().lock_owned().await;
    type_str(&mut app, "/clear");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert!(matches!(
        app.timeline.items().last(),
        Some(TimelineItem::Note(s)) if s.contains("can't clear")
    ));
}

#[test]
fn enter_on_no_match_is_noop() {
    let mut app = test_app();
    type_str(&mut app, "/blarg");
    let action = handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);
    assert_eq!(action, Action::Continue);
    // Palette stays open; user can fix the typo or Esc out.
    assert!(app.slash_palette.is_some());
    assert_eq!(app.input.lines(), &["/blarg".to_string()]);
}

#[tokio::test]
async fn enter_submits_input_to_timeline_pushes_history_and_clears() {
    let mut app = test_app();
    type_str(&mut app, "hello");
    handle_key(k(KeyCode::Enter, KeyModifiers::NONE), &mut app);

    assert_eq!(app.timeline.items().len(), 1);
    assert!(matches!(
        &app.timeline.items()[0],
        TimelineItem::User(s) if s == "hello"
    ));
    assert!(app.input_is_empty());
    assert_eq!(app.mode, AppMode::Streaming);
    assert!(app.turn_handle.is_some());
    assert_eq!(app.history.entries.back().map(String::as_str), Some("hello"));
}
