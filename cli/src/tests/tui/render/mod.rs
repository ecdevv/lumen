use super::*;
use crate::tui::test_support::test_app;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::path::PathBuf;

use crate::tui::app::{AppMode, AppState, ApprovalKind, UiMsg};

fn render_to_buffer(app: &mut AppState, w: u16, h: u16) -> Buffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn buffer_text(buf: &Buffer) -> String {
    buf.content().iter().map(ratatui::buffer::Cell::symbol).collect()
}

/// First row in `buf` whose rendered text contains `needle`.
/// Tests use this to assert relative positioning (e.g., "Lumen
/// header sits exactly N rows below the user content").
fn row_of(buf: &Buffer, needle: &str) -> Option<u16> {
    let area = buf.area;
    let w = usize::from(area.width);
    (0..area.height).find(|&row| {
        let line: String = (0..area.width)
            .filter_map(|col| {
                let idx = usize::from(row) * w + usize::from(col);
                buf.content().get(idx).map(ratatui::buffer::Cell::symbol)
            })
            .collect();
        line.contains(needle)
    })
}

#[test]
fn empty_timeline_renders_placeholder_and_help_hint() {
    let mut app = test_app();
    let buf = render_to_buffer(&mut app, 80, 20);
    let text = buffer_text(&buf);
    assert!(text.contains("Type a message to begin"));
    assert!(text.contains("Run /help for keybindings"));
}

#[test]
fn user_block_renders_you_header_and_content() {
    let mut app = test_app();
    app.timeline.push_user("hello there".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("You"));
    assert!(text.contains("hello there"));
}

#[test]
fn assistant_text_renders_lumen_header() {
    let mut app = test_app();
    app.timeline
        .push_assistant_text("the parser uses recursive descent".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("Lumen"), "expected Lumen header");
    assert!(text.contains("the parser uses recursive descent"));
}

#[test]
fn assistant_code_fences_strip_fence_markers() {
    let mut app = test_app();
    app.timeline
        .push_assistant_text("before\n```rust\nfn main() {}\n```\nafter".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("fn main() {}"));
    assert!(text.contains("before"));
    assert!(text.contains("after"));
    // Backticks are eaten by the fence detector.
    assert!(!text.contains("```"));
}

#[test]
fn running_tool_call_shows_marker_and_running_label() {
    let mut app = test_app();
    app.timeline
        .push_tool_call("c1".into(), "read".into(), r#"{"path":"a.rs"}"#.into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("read"));
    assert!(text.contains("running"));
}

#[test]
fn done_tool_call_shows_preview_and_timing() {
    let mut app = test_app();
    app.timeline.push_tool_call("c1".into(), "read".into(), "{}".into());
    std::thread::sleep(Duration::from_millis(2));
    app.timeline
        .finish_tool_call("c1", "32 lines\nmore\nstill".into(), false);
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("32 lines"));
    assert!(text.contains("+2 more lines"));
    // Either ms or s suffix should appear.
    assert!(
        text.contains("ms") || text.contains("s)"),
        "expected timing suffix, got:\n{text}"
    );
}

#[test]
fn error_tool_call_shows_error_label() {
    let mut app = test_app();
    app.timeline.push_tool_call("c1".into(), "read".into(), "{}".into());
    app.timeline.finish_tool_call("c1", "no such path".into(), true);
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("error"));
    assert!(text.contains("no such path"));
}

#[test]
fn long_args_get_truncated_with_ellipsis() {
    let huge: String = "x".repeat(200);
    assert_eq!(
        short_args(&huge).chars().filter(|c| *c != '…').count(),
        60
    );
    assert!(short_args(&huge).ends_with('…'));
}

#[test]
fn short_args_handles_utf8_boundaries() {
    let mixed: String = "é".repeat(200);
    let _ = short_args(&mixed);
}

#[test]
fn bottom_info_shows_model_cwd_and_token_placeholder() {
    // Right-aligned info line: model + cwd + token usage.
    // No mode/state segment (idle is deliberately not shown -
    // the inline streaming spinner / pin handle state).
    let mut app = test_app();
    app.cwd = PathBuf::from("/tmp/proj");
    app.session_id_short = "abc12345".into();
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(text.contains(&app.cfg.provider.model));
    assert!(text.contains("/tmp/proj") || text.contains("proj"));
    // Token placeholder until provider plumbing lands.
    assert!(text.contains("--/--"), "expected token placeholder, got:\n{text}");
}

#[test]
fn bottom_info_omits_session_id_and_lumen_prefix_and_idle() {
    // Regression bundle: the bottom bar must not regrow the
    // three things we deliberately dropped during polish -
    // hex session id, the literal "lumen" prefix, or the
    // "idle" mode segment.
    let mut app = test_app();
    app.session_id_short = "deadbeef".into();
    let buf = render_to_buffer(&mut app, 120, 14);
    let text = buffer_text(&buf);
    assert!(!text.contains("deadbeef"), "session id leaked: {text}");
    // "idle" mustn't appear in the rendered output anywhere -
    // we removed it from the status bar entirely.
    assert!(!text.contains("idle"), "idle leaked: {text}");
    // Bare " lumen ·" (the old prefix shape) shouldn't appear
    // on any row. Path-display "lumen" inside the cwd
    // (~/lumen) is legitimate, so use the prefix shape.
    let mut found_lumen_prefix = false;
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .filter_map(|x| {
                buf.cell(ratatui::layout::Position::new(x, y))
                    .map(|c| c.symbol().to_string())
            })
            .collect();
        if row.contains(" lumen ·") {
            found_lumen_prefix = true;
            break;
        }
    }
    assert!(!found_lumen_prefix, "stale 'lumen' status prefix detected");
}

#[test]
fn esc_armed_hint_appears_in_bottom_hint_slot() {
    // Quit warning now lives in the bottom status bar's left
    // half, replacing the policy/Shift+Tab hint while armed.
    use crate::tui::app::{ArmState, ArmedKey};
    let mut app = test_app();
    app.arm_state = Some(ArmState::new(ArmedKey::Esc));
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(text.contains("press Esc again to quit"));
    // While armed, the regular policy hint should NOT also
    // appear (mutually exclusive).
    assert!(
        !text.contains("Shift+Tab"),
        "policy/Shift+Tab hint should be suppressed while esc-armed"
    );
}

#[test]
fn token_usage_appears_in_bottom_hint_row_across_all_states() {
    // Tokens are the leading element of the bottom-left hint row
    // regardless of which contextual hint is showing. Pin this so
    // a future refactor of `bottom_hint_line` doesn't accidentally
    // drop tokens from one of the state branches.
    use crate::tui::app::{ArmState, ArmedKey};
    let token_placeholder = "--/-- (0%)";

    // 1. Idle (policy + Shift+Tab hint).
    let mut app = test_app();
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(
        text.contains(token_placeholder),
        "tokens must render in idle state"
    );

    // 2. Armed.
    app.arm_state = Some(ArmState::new(ArmedKey::Esc));
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(
        text.contains(token_placeholder),
        "tokens must render in armed state"
    );

    // 3. Approval pending.
    app.arm_state = None;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(crate::tui::app::PendingApproval {
        kind: crate::tui::app::ApprovalKind::Diff {
            path: std::path::PathBuf::from("f.rs"),
            diff: "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
        },
        reply: tx,
        selected: 0,
    });
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(
        text.contains(token_placeholder),
        "tokens must render in approval-pending state"
    );
}

#[test]
fn tool_call_after_user_emits_lumen_header_first() {
    // Regression: the agent's first response can be a tool call
    // (no preamble text). Without role-transition logic, the tool
    // block would appear visually under "You". Verify "Lumen"
    // appears between the user content and the tool call.
    let mut app = test_app();
    app.timeline.push_user("show me the file".into());
    app.timeline
        .push_tool_call("c1".into(), "read".into(), r#"{"path":"a"}"#.into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));

    let you_pos = text
        .find("You")
        .expect("expected 'You' in rendered output");
    let lumen_pos = text
        .find("Lumen")
        .expect("expected 'Lumen' between user and tool call");
    let tool_pos = text
        .find("read")
        .expect("expected tool name in output");
    assert!(you_pos < lumen_pos, "Lumen header should follow You");
    assert!(lumen_pos < tool_pos, "tool call should follow Lumen header");
}

#[test]
fn lumen_header_emitted_once_for_text_then_tool_then_text() {
    // Same speaker across consecutive blocks: only ONE Lumen header.
    let mut app = test_app();
    app.timeline.push_user("hi".into());
    app.timeline.push_assistant_text("Looking at the file...".into());
    app.timeline.push_tool_call("c1".into(), "read".into(), "{}".into());
    app.timeline.finish_tool_call("c1", "ok".into(), false);
    app.timeline.push_assistant_text("Done.".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 30));

    // Count "Lumen" occurrences. Should be exactly one (per turn).
    let count = text.matches("Lumen").count();
    assert_eq!(
        count, 1,
        "expected exactly one Lumen header for one turn, got {count}\n{text}"
    );
}

#[test]
fn trailing_blanks_in_assistant_text_collapse_to_inter_block_separator() {
    // Regression: model emits "...overview.\n\n" before a tool
    // call. Old behavior rendered the trailing "" line + the
    // inter-block separator as TWO blanks. New behavior drops the
    // trailing blank inside the text; only the separator remains.
    let mut app = test_app();
    app.timeline.push_user("hi".into());
    app.timeline
        .push_assistant_text("overview.\n\n".into());
    app.timeline
        .push_tool_call("c1".into(), "shell".into(), r#"{"command":"ls"}"#.into());

    let buf = render_to_buffer(&mut app, 80, 30);
    let overview = row_of(&buf, "overview.").expect("found overview row");
    let tool = row_of(&buf, "shell").expect("found tool row");
    // overview content + 1 blank separator + tool header = gap of 2.
    // Old (broken) behavior would give 3.
    assert_eq!(
        tool - overview,
        2,
        "expected exactly one blank between assistant text and tool call"
    );
}

#[test]
fn internal_blank_lines_in_assistant_text_are_preserved() {
    // Paragraph break inside the assistant text should survive.
    let mut app = test_app();
    app.timeline
        .push_assistant_text("para one\n\npara two".into());
    let buf = render_to_buffer(&mut app, 60, 20);
    let one = row_of(&buf, "para one").expect("para one row");
    let two = row_of(&buf, "para two").expect("para two row");
    // One internal blank between paragraphs -> gap of 2.
    assert_eq!(two - one, 2);
}

#[test]
fn trailing_newline_does_not_double_blank_line() {
    // Regression: text ending with `\n` previously rendered an
    // extra blank row from `split('\n')`'s trailing empty.
    // `lines()` strips a single trailing `\n` so the gap between
    // a user message ending with `\n` and the next assistant
    // block is exactly: blank separator + Lumen header.
    let mut app = test_app();
    app.timeline.push_user("hi\n".into());
    app.timeline.push_assistant_text("response".into());

    let buf = render_to_buffer(&mut app, 40, 20);
    let hi = row_of(&buf, "hi").expect("found 'hi' row");
    let resp = row_of(&buf, "response").expect("found 'response' row");

    // Expected layout between hi and response:
    //   row hi:       "  hi"
    //   row hi+1:     ""               <- inter-block blank
    //   row hi+2:     "Lumen"          <- header (role transition)
    //   row hi+3:     "  response"
    // gap = 3. With the trailing-newline bug, we'd get gap = 4.
    let gap = resp - hi;
    assert_eq!(
        gap, 3,
        "expected gap of exactly 3 rows between hi and response, got {gap}"
    );
}

#[test]
fn streaming_shows_inline_thinking_indicator_in_conversation() {
    let mut app = test_app();
    app.timeline.push_user("hi".into());
    app.mode = AppMode::Streaming;
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(
        text.contains("thinking"),
        "expected 'thinking' indicator in conversation:\n{text}"
    );
    // Spinner glyph also present alongside the label.
    let has_spinner = SPINNER_FRAMES.iter().any(|f| text.contains(f));
    assert!(has_spinner, "expected spinner glyph alongside indicator");
}

#[test]
fn idle_does_not_show_thinking_indicator() {
    let mut app = test_app();
    app.timeline.push_user("hi".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(!text.contains("thinking"));
}

#[test]
fn streaming_label_shows_inline_spinner_glyph_in_conversation() {
    // Status-bar "streaming" segment was removed - the
    // canonical streaming signal is the inline indicator in
    // the conversation pane. Verify that surface still has
    // both the spinner glyph and a "thinking" label.
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    let text = buffer_text(&render_to_buffer(&mut app, 120, 14));
    assert!(
        !text.contains("idle"),
        "idle word should not appear anywhere in rendered UI"
    );
    assert!(text.contains("thinking"), "expected inline 'thinking' label");
    let has_spinner = SPINNER_FRAMES.iter().any(|f| text.contains(f));
    assert!(has_spinner, "expected a spinner glyph in:\n{text}");
}

/// RAII guard for the NO_COLOR flag. Drops back to `false` even on
/// panic. Holds a process-wide mutex so two parallel tests can't
/// interleave a flip+restore - the global atomic doesn't help
/// across `cargo test`'s default thread pool.
struct NoColorGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl NoColorGuard {
    fn enable() -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        // `lock().unwrap()` is intentional here: a poisoned lock
        // means a prior test panicked while holding it, which is
        // information we want to surface, not paper over.
        let lock = LOCK.lock().unwrap();
        set_no_color(true);
        Self { _lock: lock }
    }
}

impl Drop for NoColorGuard {
    fn drop(&mut self) {
        set_no_color(false);
    }
}

#[test]
fn fg_helper_strips_color_when_no_color_set() {
    let g = NoColorGuard::enable();
    let s = fg(Color::Red, Modifier::BOLD);
    assert_eq!(s.fg, None);
    assert!(s.add_modifier.contains(Modifier::BOLD));
    drop(g); // restore before asserting default-color behavior
    let s2 = fg(Color::Red, Modifier::BOLD);
    assert_eq!(s2.fg, Some(Color::Red));
}

#[test]
fn help_overlay_renders_keybindings() {
    let mut app = test_app();
    app.show_help = true;
    // 40 rows: 60%x70% modal at 30 rows clips bottom binds.
    // Real terminals are typically 30-50 rows; the modal is meant
    // for "comfortable terminal" usage. This test pins that the
    // full content reaches the buffer when the frame has room.
    let text = buffer_text(&render_to_buffer(&mut app, 120, 40));
    assert!(text.contains("Keybindings"));
    assert!(text.contains("Submit message"));
    assert!(text.contains("Slash commands"));
}

#[test]
fn input_height_min_3_grows_with_lines_caps_to_protect_conversation() {
    use std::fmt::Write as _;
    let mut app = test_app();
    let w = 80; // 80-col frame
    // Empty input -> min 3 (top + bottom border + 1 content row).
    // Compact terminal-prompt shape, no padded breathing room.
    assert_eq!(input_height(&app, 30, w, 0), 3);
    // Build exactly 12 lines (no trailing newline so tui-textarea
    // doesn't add a phantom empty 13th line). Each "line N" is
    // 6 chars, well below the wrap point.
    let mut buf = String::new();
    for i in 0..11 {
        let _ = writeln!(buf, "line {i}");
    }
    buf.push_str("line 11");
    app.set_input(&buf);
    assert_eq!(app.input.lines().len(), 12, "input setup sanity");
    // Generous frame: 12 visual rows + 2 borders = 14, which
    // sits comfortably under the `MAX_INPUT_ROWS` ceiling - no
    // cap kicks in here.
    assert_eq!(input_height(&app, 30, w, 0), 14);
    // Tight frame: cap = frame_h - bottom_fixed(2) - MIN_CONV(5).
    // 12 - 2 - 5 = 5. So input shrinks to 5 rows max, even
    // though it wants 14.
    assert_eq!(input_height(&app, 12, w, 0), 5);
    // Pin-aware: the same tight frame with a 5-row approval pin
    // active should shrink the input further. 12 - 2 - 5 - 5 = 0
    // -> floored at 3.
    assert_eq!(input_height(&app, 12, w, 5), 3);
    // Tiny frames degrade to the floor of 3 instead of going negative.
    assert_eq!(input_height(&app, 4, w, 0), 3);
}

// --- wrap math --------------------------------------------------- //

#[test]
fn char_wrap_splits_at_width_boundaries() {
    assert_eq!(char_wrap("abcdefghij", 3), vec!["abc", "def", "ghi", "j"]);
    assert_eq!(char_wrap("", 5), vec![String::new()]);
    // Width 0 - pathological, fall back to no wrap.
    assert_eq!(char_wrap("hello", 0), vec!["hello"]);
    // Exact width: single visual row, no trailing empty.
    assert_eq!(char_wrap("abc", 3), vec!["abc"]);
}

#[test]
fn visual_row_count_handles_wrap_and_multiline() {
    // 200 chars / 80 = ceil = 3 rows
    let long = "x".repeat(200);
    assert_eq!(visual_row_count(&[long], 80), 3);
    // Empty logical line still occupies one visual row
    // (the cursor parks there).
    assert_eq!(visual_row_count(&[String::new()], 80), 1);
    // Mixed: 100/80=2, empty=1, 50/80=1 -> total 4
    let lines = vec!["x".repeat(100), String::new(), "y".repeat(50)];
    assert_eq!(visual_row_count(&lines, 80), 4);
}

#[test]
fn cursor_to_visual_maps_through_wrap() {
    let lines = vec!["x".repeat(100)];
    // Cursor at col 50 of a 100-char line at width 80:
    //   vrow=0 (still on first wrapped row), vcol=50.
    assert_eq!(cursor_to_visual(&lines, 0, 50, 80), (0, 50));
    // Cursor at col 100 (end of line): wraps to vrow=1, vcol=20.
    assert_eq!(cursor_to_visual(&lines, 0, 100, 80), (1, 20));
}

#[test]
fn input_height_grows_with_wrapped_visual_rows() {
    let mut app = test_app();
    let frame_w = 80u16;
    let w = usize::from(textarea_inner_width(frame_w));
    app.set_input(&"x".repeat(w * 2 + 1));
    assert_eq!(input_height(&app, 30, frame_w, 0), 5);
}

#[test]
fn input_height_keeps_row_for_cursor_past_content_on_wrap_boundary() {
    let mut app = test_app();
    let frame_w = 80u16;
    let w = usize::from(textarea_inner_width(frame_w));
    app.set_input(&"x".repeat(w));
    assert_eq!(input_height(&app, 30, frame_w, 0), 4);
}

#[test]
fn input_height_shrinks_only_when_cursor_moves_off_trailing_wrap_row() {
    let mut app = test_app();
    let frame_w = 80u16;
    let w = usize::from(textarea_inner_width(frame_w));

    app.set_input(&"x".repeat(w + 1));
    assert_eq!(input_height(&app, 30, frame_w, 0), 4);
    app.set_input(&"x".repeat(w));
    assert_eq!(
        input_height(&app, 30, frame_w, 0),
        4,
        "pane must not shrink while cursor is on the trailing wrapped row"
    );
    app.set_input(&"x".repeat(w - 1));
    assert_eq!(input_height(&app, 30, frame_w, 0), 3);
}

#[test]
fn input_height_caps_at_frame_minus_min_conv_minus_status_and_padding() {
    // Regression: the cap used to forget the 2 rows below the
    // input pane (status bar + bottom padding row). On a tight
    // frame the input would grow into the conversation's
    // minimum allocation. Pin: frame_h=15, pin=0 ->
    // available = 15 - 2 = 13, upper = 13 - 5 = 8. Many-line
    // input should clamp to 8, not 10 (the old miscalc).
    let mut app = test_app();
    let mut buf = String::new();
    for i in 0..30 {
        use std::fmt::Write as _;
        let _ = writeln!(buf, "line {i}");
    }
    app.set_input(buf.trim_end());
    assert_eq!(input_height(&app, 15, 80, 0), 8);
}

#[test]
fn input_height_capped_at_max_input_rows_on_tall_terminals() {
    // On a 60-row frame the conversation-min math would allow
    // 60 - 2 - 5 = 53 rows of input. That's pointless. The
    // hard ceiling MAX_INPUT_ROWS (12) keeps the pane bounded.
    let mut app = test_app();
    let mut buf = String::new();
    for i in 0..30 {
        use std::fmt::Write as _;
        let _ = writeln!(buf, "line {i}");
    }
    app.set_input(buf.trim_end());
    assert_eq!(input_height(&app, 60, 80, 0), MAX_INPUT_ROWS);
}

#[test]
fn input_height_subtracts_pin_height_so_conversation_keeps_its_min() {
    // With an approval pin active and 30 lines of input on a
    // 20-row frame: available = 20 - 2 (status+pad) - 5 (pin) = 13,
    // upper = 13 - 5 (min conv) = 8. Input should clamp to 8.
    let mut app = test_app();
    let mut buf = String::new();
    for i in 0..30 {
        use std::fmt::Write as _;
        let _ = writeln!(buf, "line {i}");
    }
    app.set_input(buf.trim_end());
    assert_eq!(input_height(&app, 20, 80, 5), 8);
}

#[test]
fn sanitize_strips_tabs_and_control_chars() {
    assert_eq!(sanitize_for_display("a\tb"), "a b");
    assert_eq!(sanitize_for_display("\x07bell"), " bell");
    assert_eq!(sanitize_for_display("plain text"), "plain text");
    assert_eq!(sanitize_for_display("multi\nline"), "multi line");
}

#[test]
fn markdown_inline_code_renders_in_buffer() {
    let mut app = test_app();
    app.timeline
        .push_assistant_text("run `cargo test` now".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    // The text appears verbatim - inline code is content, just with
    // styling we can't easily assert on from the buffer alone.
    assert!(text.contains("cargo test"));
    // Backticks themselves are eaten by the parser - they shouldn't
    // appear in the rendered cells.
    assert!(!text.contains('`'), "backticks should be eaten: {text}");
}

#[test]
fn markdown_bold_emphasis_strips_markers() {
    let mut app = test_app();
    app.timeline
        .push_assistant_text("really **important** detail".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("important"));
    // Asterisks gone.
    assert!(!text.contains('*'), "asterisks should be eaten: {text}");
}

#[test]
fn markdown_bullet_renders_with_bullet_glyph() {
    let mut app = test_app();
    app.timeline.push_assistant_text("- first\n- second".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("• first"));
    assert!(text.contains("• second"));
    // Hyphens should NOT appear (they're consumed as bullet markers).
    // Note: the rendered glyph is `•`, not `-`.
}

#[test]
fn markdown_heading_renders_text_without_hash_markers() {
    let mut app = test_app();
    app.timeline.push_assistant_text("# Project Overview".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("Project Overview"));
    // Hashes should not survive into the rendered cells.
    assert!(!text.contains('#'), "hashes should be stripped: {text}");
}

#[test]
fn markdown_user_content_is_not_parsed() {
    // User input should NOT have its markdown stripped - we render
    // it as literal text so users can see their own typing exactly
    // as they wrote it.
    let mut app = test_app();
    app.timeline.push_user("type **literal** stars here".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(
        text.contains("**literal**"),
        "user content should preserve markdown markers verbatim: {text}"
    );
}

#[test]
fn assistant_text_with_ansi_escape_is_sanitized() {
    // Adversarial / prompt-injection case: model emits raw ANSI
    // escape sequences in its prose. Without sanitization these
    // would land in the rendered cell symbols and the terminal
    // would interpret them as control sequences, corrupting
    // downstream rendering (same class as the `\t` drift bug).
    let mut app = test_app();
    app.timeline.push_assistant_text(
        "before\x1b[31mred\x1b[0mafter".into(),
    );
    let buf = render_to_buffer(&mut app, 80, 20);
    let text = buffer_text(&buf);
    // No raw ESC byte survives in the buffer.
    assert!(!text.contains('\x1b'), "raw ESC leaked into rendered buffer");
    // Visible content survives, just with escapes replaced by spaces.
    assert!(text.contains("before"));
    assert!(text.contains("red"));
    assert!(text.contains("after"));
}

#[test]
fn user_text_with_ansi_escape_is_sanitized() {
    // Same protection covers pasted user input that contains
    // raw escape bytes (bracketed paste delivers them verbatim).
    let mut app = test_app();
    app.timeline.push_user("hi\x1b[31mthere".into());
    let buf = render_to_buffer(&mut app, 80, 20);
    let text = buffer_text(&buf);
    assert!(!text.contains('\x1b'));
    assert!(text.contains("hi"));
    assert!(text.contains("there"));
}

#[test]
fn read_tool_preview_strips_tabs_to_avoid_terminal_drift() {
    // The read tool emits `{:>6}\t{line}`. A literal `\t` in the
    // rendered Line drifts ratatui's buffer columns out of sync
    // with the terminal's tab-stop expansion - cells past the tab
    // visually corrupt across renders.
    let mut app = test_app();
    app.timeline.push_tool_call("c1".into(), "read".into(), r#"{"path":"a"}"#.into());
    app.timeline
        .finish_tool_call("c1", "     1\t[workspace]\n     2\tnext\n".into(), false);
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(!text.contains('\t'), "raw tab leaked into rendered buffer");
    // Content still readable, just with a space instead of tab.
    assert!(text.contains("[workspace]"));
}

/// Render to a buffer, then extract whatever the selection rect
/// would copy. Used by the selection / dedent tests below.
fn extract_with_selection(
    app: &mut AppState,
    w: u16,
    h: u16,
    sel: Selection,
) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut copied = String::new();
    terminal
        .draw(|f| {
            render(f, app);
            copied = extract_selection_text(f.buffer_mut(), sel);
        })
        .unwrap();
    copied
}

/// Find the first row in the rendered buffer whose content
/// contains `needle`. Used to bound a selection to a known
/// content region without hardcoding row indices that change
/// when layout details shift.
fn find_row(app: &mut AppState, w: u16, h: u16, needle: &str) -> u16 {
    let buf = render_to_buffer(app, w, h);
    row_of(&buf, needle).unwrap_or_else(|| panic!("row containing {needle:?} not found"))
}

#[test]
fn selection_dedents_body_text_to_flush_left() {
    // Drag-select exactly the body row of a user message - no
    // header in the selection. The body line is rendered with
    // block-pad + content-prefix = 4 leading spaces; dedent
    // strips all 4 so the clipboard lands "hello world" flush.
    let mut app = test_app();
    app.timeline.push_user("hello world".into());
    let row = find_row(&mut app, 40, 14, "hello world");
    let copied = extract_with_selection(
        &mut app,
        40,
        14,
        Selection { anchor: (0, row), focus: (39, row) },
    );
    assert_eq!(copied, "hello world");
}

#[test]
fn selection_preserves_relative_indent_inside_code_block() {
    // The selection covers a code block whose lines are rendered
    // at varying indents: `fn foo() {` and `}` flush within the
    // block, but `    let x = 1;` indented 4 more spaces. After
    // dedent (by the leftmost-line's render padding), the outer
    // lines come out flush-left and `let x = 1;` keeps its
    // 4-space relative indent.
    let mut app = test_app();
    app.timeline
        .push_assistant_text("```\nfn foo() {\n    let x = 1;\n}\n```".into());
    let top = find_row(&mut app, 60, 15, "fn foo()");
    let bot = find_row(&mut app, 60, 15, "}");
    let copied = extract_with_selection(
        &mut app,
        60,
        15,
        Selection { anchor: (0, top), focus: (59, bot) },
    );
    let lines: Vec<&str> = copied.lines().collect();
    assert_eq!(lines[0], "fn foo() {");
    assert_eq!(lines[1], "    let x = 1;");
    assert_eq!(lines[2], "}");
}

#[test]
fn selection_over_blank_rows_drops_them_at_edges() {
    // Selection rect that extends above + below content into
    // padding/blank rows. Dedent + trim-blank-rows together
    // should yield exactly the content with no surrounding
    // newlines.
    let mut app = test_app();
    app.timeline.push_user("hi".into());
    let copied = extract_with_selection(
        &mut app,
        40,
        20,
        Selection { anchor: (0, 0), focus: (39, 19) },
    );
    assert!(
        !copied.starts_with('\n') && !copied.ends_with('\n'),
        "leading or trailing blank line leaked: {copied:?}"
    );
    assert!(copied.contains("hi"));
}

// --- approval modal -------------------------------------------- //

fn install_diff_pending(app: &mut AppState) {
    use crate::tui::app::PendingApproval;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Diff {
            path: PathBuf::from("src/main.rs"),
            diff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n \
                   use std::fs;\n-let x = 1;\n+let x = 42;\n"
                .into(),
        },
        reply: tx,
        selected: 0,
    });
}

fn install_shell_pending(app: &mut AppState) {
    use crate::tui::app::PendingApproval;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Shell {
            command: "rm -rf /tmp/cache".into(),
        },
        reply: tx,
        selected: 0,
    });
}

#[test]
fn diff_approval_renders_inline_with_path_diff_and_menu() {
    let mut app = test_app();
    install_diff_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 30));
    assert!(text.contains("Apply edit to"), "missing inline header");
    assert!(text.contains("src/main.rs"), "missing path");
    assert!(text.contains("-let x = 1;"), "missing removed line");
    assert!(text.contains("+let x = 42;"), "missing added line");
    assert!(text.contains("Accept"), "missing Accept option");
    assert!(text.contains("Accept all"), "missing Accept-all option");
    assert!(text.contains("Reject"), "missing Reject option");
    assert!(text.contains("(y)"), "missing y shortcut");
    assert!(text.contains("(a)"), "missing a shortcut");
    assert!(text.contains("(n)"), "missing n shortcut");
}

#[test]
fn shell_approval_renders_inline_with_command_and_menu() {
    let mut app = test_app();
    install_shell_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 20));
    assert!(text.contains("Run shell command"), "missing inline header");
    assert!(text.contains("rm -rf /tmp/cache"), "missing command");
    assert!(text.contains("Accept"));
    assert!(text.contains("Reject"));
}

#[test]
fn selected_option_gets_carrot_marker() {
    // selected = 0 (Accept) by default. The `❯` glyph should
    // appear next to "Accept" but not the other rows.
    let mut app = test_app();
    install_diff_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 30));
    // The `❯` glyph appears exactly once: at the selected row.
    assert_eq!(
        text.matches('❯').count(),
        1,
        "expected exactly one selection marker; got text:\n{text}"
    );
}

#[test]
fn selected_option_marker_follows_selection_index() {
    // Manually advance selection to "Accept all" (index 1) and
    // verify the marker moves: the row containing "Accept all"
    // should now also contain the carrot.
    use crate::tui::app::PendingApproval;
    let mut app = test_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Diff {
            path: PathBuf::from("f.rs"),
            diff: "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
        },
        reply: tx,
        selected: 1,
    });
    let buf = render_to_buffer(&mut app, 100, 30);
    let accept_all_row = row_of(&buf, "Accept all").expect("found Accept all row");
    let marker_row = row_of(&buf, "❯").expect("found marker row");
    assert_eq!(marker_row, accept_all_row, "marker should land on selected row");
}

#[test]
fn bottom_hint_shows_policy_and_shift_tab_in_default_state() {
    let mut app = test_app();
    let text = buffer_text(&render_to_buffer(&mut app, 100, 20));
    // Default policy = Never -> label "ask edits".
    assert!(text.contains("ask edits"), "missing policy label");
    assert!(text.contains("Shift+Tab"), "missing shift-tab hint");
    assert!(text.contains("cycle"), "missing cycle hint");
}

#[test]
fn bottom_hint_flips_to_navigation_during_approval() {
    let mut app = test_app();
    install_diff_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 30));
    // Approval up: bottom hint shows navigation, not the mode label.
    assert!(text.contains("navigate"), "missing nav hint");
    assert!(text.contains("Enter"), "missing Enter hint");
    // Pin the bottom-row exclusivity: the bottom-bar row that
    // contains the nav hint must NOT also contain "ask edits".
    // Other rows in the frame (the pin's menu) are unaffected.
    let buf = render_to_buffer(&mut app, 100, 30);
    let hint_row = row_of(&buf, "navigate").expect("found nav-hint row");
    let row_text = row_text_at(&buf, hint_row);
    assert!(
        !row_text.contains("ask edits"),
        "policy label leaked into nav-hint row: {row_text}"
    );
}

/// Extract one row's text from `buf` as a contiguous string.
fn row_text_at(buf: &Buffer, row: u16) -> String {
    let w = buf.area.width;
    (0..w)
        .filter_map(|col| buf.cell(Position::new(col, row)).map(|c| c.symbol().to_string()))
        .collect()
}

#[test]
fn approval_preview_replaces_thinking_spinner() {
    // While a tool awaits a verdict, the model isn't generating.
    // The "Lumen is thinking…" spinner would mislead, so it
    // must be suppressed in favor of the preview.
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    install_diff_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 30));
    assert!(
        !text.contains("thinking"),
        "thinking spinner should be hidden while approval pending; got: {text}"
    );
}

#[test]
fn approval_preview_sanitizes_control_chars_in_diff() {
    // Prompt-injection hardening: tab / ESC inside a diff line
    // must not leak into the rendered buffer (where it would
    // desync ratatui's cell math with the terminal's expansion).
    use crate::tui::app::PendingApproval;
    let mut app = test_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.pending_approval = Some(PendingApproval {
        kind: ApprovalKind::Diff {
            path: PathBuf::from("f.rs"),
            diff: "--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\x1b[31m\n+new\tdata\n".into(),
        },
        reply: tx,
        selected: 0,
    });
    let buf = render_to_buffer(&mut app, 80, 30);
    let text = buffer_text(&buf);
    assert!(!text.contains('\x1b'), "raw ESC leaked into preview");
    assert!(!text.contains('\t'), "raw tab leaked into preview");
}

// --- polish-pass coverage --------------------------------------- //

#[test]
fn preview_skips_shell_exit_and_separator_markers() {
    // Shell output: `exit: 0` then `--- stdout ---` then actual
    // content. The preview should surface the content line, not
    // the framing markers.
    let p = preview("exit: 0\n--- stdout ---\nhello world\n");
    assert!(p.starts_with("hello world"), "got: {p}");
    // 3 lines total, 1 displayed -> 2 more.
    assert!(p.contains("+2 more lines"), "got: {p}");
}

#[test]
fn preview_falls_back_to_first_nonblank_when_all_markers() {
    // Pathological: every line is a marker. Fall back to the
    // first non-blank line rather than empty.
    let p = preview("exit: 7\n--- stderr ---\n");
    assert!(p.starts_with("exit: 7"), "got: {p}");
}

#[test]
fn error_notes_render_with_red_marker() {
    // A note starting with "agent error:" should render with
    // the ✗ marker, distinguishing it from the dim · footer.
    let mut app = test_app();
    app.timeline.push_note("agent error: connection refused".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("✗"), "expected error marker, got:\n{text}");
    assert!(text.contains("connection refused"));
}

#[test]
fn info_notes_render_with_dim_bullet_marker() {
    // Routine notes ("Cooked for X", "cancelled by user") keep
    // the dim `·` marker - no red, no escalation.
    let mut app = test_app();
    app.timeline.push_note("cancelled by user".into());
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("·"));
    assert!(!text.contains("✗"));
}

#[test]
fn format_tool_action_maps_each_builtin_tool() {
    // The dynamic streaming-spinner label adapts per tool.
    let cwd = std::path::Path::new(".");
    assert!(format_tool_action("read", r#"{"path":"a.rs"}"#, cwd).starts_with("Reading"));
    assert!(format_tool_action("write", r#"{"path":"a.rs"}"#, cwd).starts_with("Writing"));
    assert!(format_tool_action("edit", r#"{"path":"a.rs"}"#, cwd).starts_with("Editing"));
    assert!(format_tool_action("grep", r#"{"pattern":"foo"}"#, cwd).contains("foo"));
    let shell_label = format_tool_action("shell", r#"{"command":"ls -la"}"#, cwd);
    assert!(shell_label.starts_with("Running"), "got: {shell_label}");
    assert!(shell_label.contains("ls -la"));
}

#[test]
fn format_tool_action_falls_back_on_unknown_tool() {
    // Unknown tools get a generic verb so the spinner still says
    // something meaningful rather than crashing or going blank.
    let label = format_tool_action("custom_tool", "{}", std::path::Path::new("."));
    assert!(!label.is_empty());
    assert!(label.ends_with('…'));
}

#[test]
fn streaming_spinner_uses_active_tool_label_when_a_tool_is_active() {
    // While `app.active_tool` is set (between ToolCallStart and
    // the next AssistantText/TurnEnd), the spinner label adapts
    // to the tool's action rather than the generic "thinking…".
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    app.active_tool = Some(("read".into(), r#"{"path":"src/lib.rs"}"#.into()));
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("Reading"), "expected dynamic label, got:\n{text}");
    assert!(text.contains("src/lib.rs"));
    assert!(!text.contains("thinking"), "generic label should be suppressed");
}

#[test]
fn active_tool_clears_on_tool_call_end() {
    // The spinner reflects "what's happening now," not "what
    // just happened." End is the canonical clear point so the
    // label returns to "thinking…" immediately when the tool
    // finishes, even though the model usually takes seconds
    // to respond to the tool result.
    //
    // The "fast tool never gets a Running frame" problem is
    // solved at the event-loop layer (force-render after Start
    // in `tui::mod::run`), not by persisting state past End.
    use lumen_core::AgentEvent;
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::ToolCallStart {
        id: "c1".into(),
        name: "read".into(),
        arguments: r#"{"path":"src/lib.rs"}"#.into(),
    }));
    assert!(app.active_tool.is_some(), "active_tool set on Start");
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::ToolCallEnd {
        id: "c1".into(),
        result: "32 lines".into(),
        is_error: false,
    }));
    assert!(
        app.active_tool.is_none(),
        "active_tool should clear on End so spinner stops claiming an old tool is still running"
    );
}

#[test]
fn streaming_spinner_label_returns_to_thinking_after_tool_end() {
    // Render-level companion to the above: after End, the
    // spinner label must show the generic "thinking…" again.
    use lumen_core::AgentEvent;
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::ToolCallStart {
        id: "c1".into(),
        name: "read".into(),
        arguments: r#"{"path":"a"}"#.into(),
    }));
    app.apply_ui_msg(UiMsg::Agent(AgentEvent::ToolCallEnd {
        id: "c1".into(),
        result: "ok".into(),
        is_error: false,
    }));
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(
        text.contains("thinking"),
        "spinner should return to generic label after End, got:\n{text}"
    );
    assert!(
        !text.contains("Reading"),
        "dynamic label should be gone after End"
    );
}

#[test]
fn rejected_tool_call_renders_red_icon() {
    // A rejection lives in `Done(result)` (not `Error`) so the
    // model gets a clean message, but the renderer still uses
    // the red treatment so the user can tell the operation
    // didn't apply. Detection is by `REJECTION_PREFIX`.
    let mut app = test_app();
    app.timeline
        .push_tool_call("c1".into(), "write".into(), r#"{"path":"a"}"#.into());
    app.timeline.finish_tool_call(
        "c1",
        format!(
            "{} write to /tmp/a was NOT performed. File unchanged.",
            lumen_core::REJECTION_PREFIX
        ),
        false,
    );
    let buf = render_to_buffer(&mut app, 100, 20);
    let row = row_of(&buf, "write(").expect("write row");
    let mut dot_color = None;
    for x in 0..buf.area.width {
        if let Some(cell) = buf.cell(ratatui::layout::Position::new(x, row))
            && cell.symbol() == "●"
        {
            dot_color = Some(cell.fg);
            break;
        }
    }
    assert_eq!(dot_color, Some(Color::Red), "rejected ● should be red");
}

#[test]
fn successful_tool_call_still_renders_green_icon() {
    // The rejection-detection heuristic must NOT bleed into
    // ordinary success results - "wrote 12 bytes ..." stays green.
    let mut app = test_app();
    app.timeline
        .push_tool_call("c1".into(), "write".into(), r#"{"path":"a"}"#.into());
    app.timeline
        .finish_tool_call("c1", "wrote 12 bytes to /tmp/a".into(), false);
    let buf = render_to_buffer(&mut app, 100, 20);
    let row = row_of(&buf, "write(").expect("write row");
    let mut dot_color = None;
    for x in 0..buf.area.width {
        if let Some(cell) = buf.cell(ratatui::layout::Position::new(x, row))
            && cell.symbol() == "●"
        {
            dot_color = Some(cell.fg);
            break;
        }
    }
    assert_eq!(dot_color, Some(Color::Green), "success ● should stay green");
}

#[test]
fn streaming_spinner_falls_back_to_generic_thinking_between_tool_calls() {
    // No Running tool on the timeline = pre/post tool dispatch.
    // Generic label is correct here.
    let mut app = test_app();
    app.mode = AppMode::Streaming;
    let text = buffer_text(&render_to_buffer(&mut app, 80, 20));
    assert!(text.contains("thinking"));
}

#[test]
fn glyph_fallback_uses_ascii_arrow_in_approval_menu() {
    // unicode_glyphs=false swaps `❯` for `>` so legacy fonts
    // render cleanly.
    let mut app = test_app();
    app.cfg.ui.unicode_glyphs = false;
    install_diff_pending(&mut app);
    let text = buffer_text(&render_to_buffer(&mut app, 100, 30));
    assert!(!text.contains('❯'), "unicode arrow leaked under fallback");
    // The `>` appears in the menu and in the input pane prompt;
    // a bare contains-check is sufficient to confirm something
    // rendered with it.
    assert!(text.contains('>'));
}

#[test]
fn approval_pin_height_scales_by_kind() {
    let diff_kind = ApprovalKind::Diff {
        path: PathBuf::from("a"),
        diff: String::new(),
    };
    let shell_kind = ApprovalKind::Shell {
        command: "ls".into(),
    };
    // Diff: 1 (border) + 1 (header) + 3 (options) = 5
    assert_eq!(approval_pin_height(&diff_kind), 5);
    // Shell: 1 (border) + 1 (header) + 1 (command) + 2 (options) = 5
    assert_eq!(approval_pin_height(&shell_kind), 5);
}

#[test]
fn approval_pin_renders_above_policy_hint_not_inside_conversation() {
    // Pin lives in its own layout slice between conversation and
    // policy-hint. The header ("Apply edit to ...") should sit
    // BELOW the diff body row in the rendered frame, not above
    // it (the body is appended at the end of the conversation,
    // the pin lives below).
    let mut app = test_app();
    install_diff_pending(&mut app);
    let buf = render_to_buffer(&mut app, 100, 30);
    let body_row = row_of(&buf, "-let x = 1;").expect("diff body row");
    let header_row = row_of(&buf, "Apply edit to").expect("pin header row");
    assert!(
        header_row > body_row,
        "pin header should render below the diff body, got header={header_row}, body={body_row}"
    );
}

#[test]
fn format_duration_picks_appropriate_unit() {
    assert_eq!(format_duration(Duration::from_millis(12)), "12ms");
    assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
    let s = format_duration(Duration::from_millis(3400));
    assert!(s.contains("3.4") && s.ends_with('s'), "got {s}");
    let m = format_duration(Duration::from_secs(125));
    assert!(m.contains("2m") && m.contains("5s"), "got {m}");
}
