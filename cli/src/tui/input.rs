//! Terminal event to [`Action`] mapping.
//!
//! Key dispatch table (state-dependent):
//!
//! | Key        | Streaming           | Idle + content (1st) | Idle + content (2nd same key) | Idle + empty (1st) | Idle + empty (2nd same key) |
//! |------------|---------------------|----------------------|-------------------------------|--------------------|-----------------------------|
//! | `Esc`      | Cancel turn         | Arm + status hint    | Clear input                   | Arm + status hint  | Quit                        |
//! | `Ctrl+C`   | Cancel turn         | Clear input          | -                             | Arm + status hint  | Quit                        |
//! | `Ctrl+D`   | Quit                | Quit                 | -                             | Quit               | -                           |
//! | `Up/Down`  | (no-op)             | Cursor in textarea (multi-line) | -                  | Recall history     | Recall history              |
//! | `Ctrl+Z`   | (no-op stream-side) | Undo                 | -                             | Undo (no-op)       | -                           |
//! | `Enter`    | (no-op)             | Submit               | -                             | (no-op on empty)   | -                           |
//!
//! [`AppState::arm_state`] persists across consecutive presses of the
//! same chord key (Esc or Ctrl+C); pressing any other key resets it.
//! It also auto-expires after [`super::app::ARM_TIMEOUT`] so a stale
//! "press again" hint can't linger forever.

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use lumen_core::{Agent, AutoApply, Verdict};
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tui_textarea::CursorMove;

use super::app::{
    Action, AppMode, AppState, ApprovalOption, ArmState, ArmedKey, Selection, UiMsg,
    approval_options,
};
use super::slash::SlashPalette;

/// Translate one terminal [`Event`] into an [`Action`].
pub fn handle_event(ev: &Event, app: &mut AppState) -> Action {
    match ev {
        Event::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..
        }) => Action::Continue,
        Event::Key(k) => handle_key(*k, app),
        Event::Mouse(m) => {
            handle_mouse(*m, app);
            Action::Continue
        }
        Event::Paste(text) => {
            // Bracketed paste: the terminal delivered the whole pasted
            // chunk as one event. Insert verbatim - tui-textarea splits
            // embedded \n into lines so multi-line paste lands correctly.
            app.input.insert_str(text);
            Action::Continue
        }
        _ => Action::Continue,
    }
}

/// Mouse handling:
///   * Wheel-up/down scroll the conversation. Selection follows
///     content via the post-clamp delta in `render_conversation`.
///   * Left-button down/drag/up drives our custom selection. Click
///     without drag clears any existing selection.
///   * On drag-then-release, the selection auto-copies to the system
///     clipboard via OSC 52 when [`UiConfig::auto_copy_on_select`] is
///     on (default). There's no dedicated copy chord because every
///     modifier-based candidate (Ctrl+Shift+C, Alt+C) breaks in some
///     terminal/OS combo we care about; the act of selecting *is*
///     the copy gesture.
fn handle_mouse(m: MouseEvent, app: &mut AppState) {
    const WHEEL_STEP: usize = 3;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Start fresh selection. Any pending copy from a prior
            // selection is voided - the user clearly moved on.
            app.selection = Some(Selection {
                anchor: (m.column, m.row),
                focus: (m.column, m.row),
            });
            app.render.copy_pending = false;
            app.render.clipboard_pending = None;
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(sel) = app.selection.as_mut() {
                sel.focus = (m.column, m.row);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some(sel) = app.selection.as_ref() {
                if sel.anchor == sel.focus {
                    // Click without drag - treat as "clear selection."
                    app.selection = None;
                } else if app.cfg.ui.auto_copy_on_select {
                    // Drag-then-release while auto-copy is on (default):
                    // the next render extracts text from the post-paint
                    // buffer and the main loop OSC-52s it. Users who
                    // set `auto_copy_on_select = false` keep the
                    // selection visible without touching the clipboard.
                    tracing::debug!("drag released; marking copy_pending (auto-copy)");
                    app.render.copy_pending = true;
                }
            }
        }
        MouseEventKind::ScrollUp => {
            // Selection follows content under scroll. The actual shift
            // is applied in `render_conversation` based on the
            // *post-clamp* delta - doing it here would over-shift at
            // the bounds (scroll_offset can be requested beyond
            // bottom_anchor and gets clamped during render).
            app.scroll_offset = app.scroll_offset.saturating_add(WHEEL_STEP);
        }
        MouseEventKind::ScrollDown => {
            app.scroll_offset = app.scroll_offset.saturating_sub(WHEEL_STEP);
        }
        _ => {}
    }
}

/// PgUp / PgDn step size in rows. Half a typical 24-row terminal feels
/// natural for "skim back through the conversation."
const PAGE_STEP: usize = 10;

fn handle_key(k: KeyEvent, app: &mut AppState) -> Action {
    // Trace every key event so users can diagnose why a chord didn't
    // hit a binding (e.g. terminal swallowing Ctrl+Shift+C). Enable
    // with `LUMEN_LOG=trace`.
    tracing::trace!(?k, "handle_key");

    // Approval prompt is the highest-priority intercept. While a tool
    // awaits a verdict, the user navigates the 3-option menu with
    // arrow keys and confirms with Enter; y / a / n are letter
    // shortcuts that apply a choice immediately. Everything else
    // is swallowed so typing into the input box mid-prompt can't
    // surprise on dismissal. Ctrl+C is intentionally NOT routed to
    // cancel-turn here - rejecting the prompt is the expected
    // mental shape ("answer the question"); the user can Ctrl+C
    // again afterwards to abort the full turn.
    if app.pending_approval.is_some() {
        // Universal escape hatches still fire during an approval
        // prompt - swallowing them would strand the user with no
        // way out except answering the prompt. Three distinct
        // semantics here:
        //   * Esc   -> reject this one prompt (turn continues)
        //     [handled inside `handle_approval_key`]
        //   * Ctrl+C -> cancel the entire turn (clears the prompt
        //              via `cancel_turn`'s pending_approval = None)
        //   * Ctrl+D -> quit lumen entirely
        if is_ctrl_d(k) {
            return Action::Quit;
        }
        if is_ctrl_c(k) {
            cancel_turn(app);
            return Action::Continue;
        }
        handle_approval_key(k, app);
        return Action::Continue;
    }

    // Detach from history-browse mode if the user has edited the
    // recalled entry. Once the buffer diverges from what we pulled
    // out of history, treat subsequent Up/Down as normal cursor
    // navigation (with edge-nudge -> history at the boundary), not
    // continued rotation through history. Matches bash/zsh /
    // fish behavior. Pure cursor movement (Left, Right, Home,
    // End) doesn't modify the buffer, so history mode survives it.
    detach_from_history_if_edited(app);

    // Help overlay is modal. Esc or Ctrl+C dismisses it; Ctrl+D
    // bypasses and quits the app (same universal-quit semantics as
    // the approval prompt). Everything else is swallowed so typing
    // into the box behind the overlay can't accidentally fire
    // actions (and `?` doesn't bounce-toggle).
    if app.show_help {
        if is_ctrl_d(k) {
            return Action::Quit;
        }
        if matches!(k.code, KeyCode::Esc) || is_ctrl_c(k) {
            app.show_help = false;
        }
        return Action::Continue;
    }

    // Modal precedence is "innermost interaction wins". The model
    // picker can be opened *from inside* the settings overlay
    // (activating the `provider.model` field), so when both are
    // up the picker takes keys until dismissed; settings remains
    // visible behind it and resumes once the picker closes.
    if let Some(action) = super::model_picker::handle_picker_key(k, app) {
        return action;
    }

    // Settings overlay intercept. Same Ctrl+D bypass as the
    // other modals. Nav mode and edit mode have distinct key
    // handling; the helper module owns the full dispatch table.
    if let Some(action) = super::settings::handle_modal_key(k, app) {
        return action;
    }

    // Slash palette intercept: claim nav/commit keys when open.
    // See `slash::handle_palette_key` for the full dispatch.
    if let Some(action) = super::slash::handle_palette_key(k, app) {
        return action;
    }

    // Snapshot the live armed state (auto-expires past ARM_TIMEOUT),
    // then disarm in two cases: (a) the previous arm has already
    // expired, or (b) this keypress is neither Esc nor Ctrl+C - the
    // only two keys that participate in chord progression. The Esc /
    // Ctrl+C handlers below re-arm or confirm as appropriate.
    let prev_armed = app.armed_key();
    let is_chord_key = matches!(k.code, KeyCode::Esc) || is_ctrl_c(k);
    if prev_armed.is_none() || !is_chord_key {
        app.arm_state = None;
    }

    let action = match (k.code, k.modifiers) {
        // `/` on empty input opens the slash palette. The keystroke
        // also forwards to the textarea (input becomes "/") so the
        // user's typing shows up in the familiar gutter. Subsequent
        // chars are picked up by the catch-all and the post-pass
        // keeps the palette in sync. Mid-message `/` falls through
        // and inserts a literal `/`.
        // `palette.is_none()` is implied by `input_is_empty()`: an
        // open palette guarantees at least `/` is in the buffer.
        (KeyCode::Char('/'), KeyModifiers::NONE) if app.input_is_empty() => {
            app.slash_palette = Some(SlashPalette::new());
            app.input.input(k);
            Action::Continue
        }

        // Ctrl+D always quits.
        (KeyCode::Char('d'), KeyModifiers::CONTROL) => Action::Quit,

        // Shift+Tab cycles the approval policy: Never <-> Safe.
        // Matches Claude Code's mode-cycle chord (an "Always" /
        // auto-shell tier isn't planned for v0.1; per-command
        // allowlisting is the future shape). Two encodings to
        // handle across terminals:
        //   * `KeyCode::BackTab` - traditional xterm CSI-Z encoding
        //   * `KeyCode::Tab` with `SHIFT` - Kitty keyboard protocol
        //     (we push DISAMBIGUATE_ESCAPE_CODES in setup_terminal,
        //     so on kitty/wezterm/foot we get this shape)
        (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
            cycle_auto_apply(app);
            Action::Continue
        }

        // Esc and Ctrl+C share the cancel-on-streaming step, then
        // diverge: Esc requires a double-tap to clear *or* quit;
        // Ctrl+C clears on a single tap (idle+content) but needs a
        // double-tap to quit (idle+empty).
        (KeyCode::Esc, _) => handle_esc_chord(app, prev_armed),
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => handle_ctrl_c_chord(app, prev_armed),

        // Plain Enter submits. Shift+Enter and Alt+Enter both fall
        // through to the textarea, which inserts a newline. Alt+Enter
        // is the *reliable* newline across terminals - most don't
        // encode the Shift modifier on Enter, so Shift+Enter only
        // works on kitty/wezterm/foot with the kitty keyboard protocol.
        (KeyCode::Enter, m) if !m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
            submit(app);
            Action::Continue
        }

        // Visual-row-aware Up/Down. The buffer renders with char-wrap
        // at the pane edge, so a long pasted single line occupies
        // several visual rows - Up must move one *visual* row up,
        // not jump to the start of the logical line. See
        // `handle_up` / `handle_down` for the full dispatch
        // (wrap-continuation move / cross-logical-line move /
        // edge-nudge / history-recall, in priority order).
        (KeyCode::Up, KeyModifiers::NONE) => {
            handle_up(app);
            Action::Continue
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            handle_down(app);
            Action::Continue
        }

        // PgUp / PgDn scroll the conversation. PgUp increments
        // scroll_offset (move view *away* from the bottom toward older
        // content), PgDn decrements toward 0 (anchored at bottom).
        // Selection-row shift is applied during render based on the
        // post-clamp scroll delta (see `render_conversation`).
        (KeyCode::PageUp, KeyModifiers::NONE) => {
            app.scroll_offset = app.scroll_offset.saturating_add(PAGE_STEP);
            Action::Continue
        }
        (KeyCode::PageDown, KeyModifiers::NONE) => {
            app.scroll_offset = app.scroll_offset.saturating_sub(PAGE_STEP);
            Action::Continue
        }

        // Ctrl+Backspace -> word-back-delete. tui-textarea binds this
        // action to Ctrl+W and Alt+Backspace by default but not to
        // Ctrl+Backspace, so we re-emit the chord as Ctrl+W so the
        // operation lands in tui-textarea's input pathway (and undo
        // stack) identically to the native chords. Requires the Kitty
        // Keyboard Protocol (pushed in `super::setup_terminal`) to
        // receive `Backspace+CONTROL` distinctly; terminals without
        // protocol support typically arrive as plain backspace anyway.
        (KeyCode::Backspace, KeyModifiers::CONTROL) => {
            let synthetic = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
            app.input.input(synthetic);
            Action::Continue
        }

        // Ctrl+Z -> undo. tui-textarea's default emacs binding for undo
        // is Ctrl+_ (Ctrl+Underscore), which most users won't discover.
        // Routing Ctrl+Z to TextArea::undo gives the Windows/editor
        // convention everyone actually expects.
        (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
            let _ = app.input.undo();
            Action::Continue
        }

        // Anything else (chars, arrows over multi-line, backspace,
        // Ctrl+Z=undo, Ctrl+W, etc.) is editing input - let
        // `tui-textarea` decide.
        _ => {
            app.input.input(k);
            Action::Continue
        }
    };

    // Post-pass: the main match may have mutated input (typing,
    // backspace, undo). Reconcile palette + picker state with the
    // new input - slash closes when the leading `/` is gone;
    // model picker clamps its selected row to the live filter.
    super::slash::sync_palette(app);
    super::model_picker::sync_picker(app);

    action
}

/// Esc progression on each press:
///
/// 1. **Streaming** -> cancel the in-flight turn (single tap; the running
///    task is the expensive thing). Input draft is preserved.
/// 2. **Idle, non-empty input** -> double-tap to clear. First tap arms,
///    second tap (within [`ARM_TIMEOUT`]) wipes the buffer.
/// 3. **Idle, empty input** -> double-tap to quit. First tap arms,
///    second tap exits.
fn handle_esc_chord(app: &mut AppState, prev_armed: Option<ArmedKey>) -> Action {
    if app.mode == AppMode::Streaming {
        cancel_turn(app);
        return Action::Continue;
    }
    if !app.input_is_empty() {
        if prev_armed == Some(ArmedKey::Esc) {
            app.reset_input();
            app.arm_state = None;
        } else {
            app.arm_state = Some(ArmState::new(ArmedKey::Esc));
        }
        return Action::Continue;
    }
    // Idle + empty.
    if prev_armed == Some(ArmedKey::Esc) {
        Action::Quit
    } else {
        app.arm_state = Some(ArmState::new(ArmedKey::Esc));
        Action::Continue
    }
}

/// Ctrl+C progression on each press:
///
/// 1. **Streaming** -> cancel the in-flight turn.
/// 2. **Idle, non-empty input** -> clear on a single tap (matches bash;
///    Ctrl+C is the canonical "discard line" gesture).
/// 3. **Idle, empty input** -> double-tap to quit. First tap arms,
///    second tap exits.
fn handle_ctrl_c_chord(app: &mut AppState, prev_armed: Option<ArmedKey>) -> Action {
    if app.mode == AppMode::Streaming {
        cancel_turn(app);
        return Action::Continue;
    }
    if !app.input_is_empty() {
        app.reset_input();
        app.arm_state = None;
        return Action::Continue;
    }
    // Idle + empty.
    if prev_armed == Some(ArmedKey::CtrlC) {
        Action::Quit
    } else {
        app.arm_state = Some(ArmState::new(ArmedKey::CtrlC));
        Action::Continue
    }
}

/// Wrap-aware Up handler. Priority:
///
/// 1. **History-browse mode** (`history.cursor.is_some()`) - Up walks
///    older, regardless of where the cursor sits within the recalled
///    entry. Matches bash/fish: once cycling history, the meaning of
///    Up is fixed.
/// 2. **Wrap-continuation move** - if the current logical line has a
///    wrap subrow *above* the cursor (i.e., cursor isn't on the first
///    visual subrow of its logical line), jump up one visual row in
///    place, preserving visual column. This is the case that the old
///    logical-row-only dispatcher got wrong: a 200-char pasted line is
///    one logical row but three visual rows, and Up was jumping to
///    column 0 of the whole line.
/// 3. **Cross-logical move** - if there's a previous logical line,
///    jump to its *last* visual subrow at the same visual column
///    (clamped to that subrow's content length).
/// 4. **Edge-nudge / recall** - we're at absolute visual top of the
///    buffer. If the visual column > 0, nudge to col 0 (fish-style).
///    If already at (0, 0), walk older history.
fn handle_up(app: &mut AppState) {
    if app.history.cursor.is_some() {
        recall_older(app);
        return;
    }
    let width = usize::from(app.render.last_textarea_width);
    let lines = app.input.lines();
    let (cur_row, cur_col) = app.input.cursor();

    // Width 0 (no render has happened yet, or pathological tiny
    // terminal) falls back to logical-row semantics so test
    // fixtures and edge cases stay sane.
    if width == 0 {
        if cur_row == 0 && cur_col == 0 {
            recall_older(app);
        } else if cur_row == 0 {
            app.input.move_cursor(CursorMove::Head);
        } else {
            app.input.move_cursor(CursorMove::Up);
        }
        return;
    }

    let (vrow_u16, vcol_u16) =
        super::render::cursor_to_visual(lines, cur_row, cur_col, width);
    let vrow = usize::from(vrow_u16);
    if vrow == 0 {
        // Absolute visual top. The wrap-continuation branch already
        // returned for everything else on logical row 0; reaching
        // here means cur_col < width.
        if cur_col == 0 {
            recall_older(app);
        } else {
            app.input.move_cursor(CursorMove::Head);
        }
        return;
    }
    let (target_row, target_col) = super::render::visual_to_logical(
        lines,
        vrow - 1,
        usize::from(vcol_u16),
        width,
    );
    app.input.move_cursor(CursorMove::Jump(
        u16::try_from(target_row).unwrap_or(u16::MAX),
        u16::try_from(target_col).unwrap_or(u16::MAX),
    ));
}

/// Wrap-aware Down handler. Mirror of [`handle_up`]:
///
/// 1. History-browse mode -> recall newer.
/// 2. Wrap-continuation move below within or across logical lines.
/// 3. End-nudge -> recall newer at end-of-buffer.
fn handle_down(app: &mut AppState) {
    if app.history.cursor.is_some() {
        recall_newer(app);
        return;
    }
    let width = usize::from(app.render.last_textarea_width);
    let lines = app.input.lines();
    let (cur_row, cur_col) = app.input.cursor();
    let last_row = lines.len().saturating_sub(1);
    let last_col = lines.get(last_row).map_or(0, |l| l.chars().count());

    if width == 0 {
        // Logical-row fallback path.
        if cur_row == last_row && cur_col == last_col {
            recall_newer(app);
        } else if cur_row == last_row {
            app.input.move_cursor(CursorMove::End);
        } else {
            app.input.move_cursor(CursorMove::Down);
        }
        return;
    }

    let total_vrows = super::render::visual_row_count(lines, width);
    let (vrow_u16, vcol_u16) =
        super::render::cursor_to_visual(lines, cur_row, cur_col, width);
    let vrow = usize::from(vrow_u16);
    if vrow + 1 >= total_vrows {
        // At absolute visual bottom (or on the imaginary row past
        // the last content row, when cur_col sits at a wrap
        // boundary of the last line).
        if cur_row == last_row && cur_col == last_col {
            recall_newer(app);
        } else {
            app.input.move_cursor(CursorMove::End);
        }
        return;
    }
    let (target_row, target_col) = super::render::visual_to_logical(
        lines,
        vrow + 1,
        usize::from(vcol_u16),
        width,
    );
    app.input.move_cursor(CursorMove::Jump(
        u16::try_from(target_row).unwrap_or(u16::MAX),
        u16::try_from(target_col).unwrap_or(u16::MAX),
    ));
}

fn cancel_turn(app: &mut AppState) {
    if let Some(handle) = app.turn_handle.take() {
        // `abort` drops the spawned future, which drops the provider
        // stream, which cancels the in-flight HTTP request. Any tools
        // mid-dispatch get killed via `kill_on_drop` (shell) or
        // future-cancellation (fs IO).
        handle.abort();
    }
    // Drop any pending approval. The tool's `rx.await` returns
    // `Err(RecvError)` once the sender drops, which
    // `TuiApprovalGate::ask` maps to `Verdict::Reject` - but the
    // tool task itself is being aborted anyway, so the verdict is
    // moot. Clearing here keeps the modal from lingering with stale
    // data.
    app.pending_approval = None;
    // Same idea for the dynamic streaming label - the aborted tool
    // shouldn't keep its name in the spinner.
    app.active_tool = None;
    // Any pending double-tap chord is moot now; the press that got
    // us here resolved as "cancel," not as a chord step.
    app.arm_state = None;
    app.timeline.push_note("cancelled by user".into());
    app.mode = AppMode::Idle;
}

/// Send a verdict back to the tool awaiting the reply, then clear
/// the pending state so the inline preview dismisses on the next
/// frame.
fn send_verdict(app: &mut AppState, verdict: Verdict) {
    if let Some(pending) = app.pending_approval.take() {
        // `send` fails if the receiver was dropped (tool task
        // aborted in the meantime). Swallowing the error is fine:
        // nothing's waiting for the verdict.
        let _ = pending.reply.send(verdict);
    }
}

/// Toggle the approval policy via Shift+Tab. Visible confirmation
/// is the policy-hint row's label change - no timeline note (rapid
/// cycling would otherwise spam the conversation). Tracing captures
/// the change for debug builds / `LUMEN_LOG=info`.
fn cycle_auto_apply(app: &mut AppState) {
    let next = app.auto_apply().next();
    app.set_auto_apply(next);
    tracing::info!(mode = ?next, "approval mode cycled via Shift+Tab");
}

/// Dispatch one keystroke received while an approval prompt is up.
/// Arrow keys move the selection within the kind-specific option
/// table; Enter confirms the highlighted option; letter keys are
/// shortcuts that apply the option whose shortcut matches.
/// Everything else is swallowed.
fn handle_approval_key(k: KeyEvent, app: &mut AppState) {
    match k.code {
        KeyCode::Up => {
            if let Some(p) = app.pending_approval.as_mut() {
                p.selected = p.selected.saturating_sub(1);
            }
        }
        KeyCode::Down => {
            if let Some(p) = app.pending_approval.as_mut() {
                let max = approval_options(&p.kind).len().saturating_sub(1);
                p.selected = (p.selected + 1).min(max);
            }
        }
        KeyCode::Enter => {
            let opt = app.pending_approval.as_ref().and_then(|p| {
                approval_options(&p.kind)
                    .get(p.selected)
                    .map(|(o, _, _)| *o)
            });
            if let Some(opt) = opt {
                apply_approval_option(app, opt);
            }
        }
        KeyCode::Esc => apply_approval_option(app, ApprovalOption::Reject),
        // Letter shortcut: look it up in the active kind's option
        // table. A char with no matching shortcut (e.g. `a` on a
        // shell prompt) is silently dropped - that's honest:
        // there's no "Accept all" for shell yet, so the user
        // gets no false-positive action.
        KeyCode::Char(c) => {
            let lower = c.to_ascii_lowercase();
            let opt = app.pending_approval.as_ref().and_then(|p| {
                approval_options(&p.kind)
                    .iter()
                    .find(|(_, _, key)| *key == lower)
                    .map(|(o, _, _)| *o)
            });
            if let Some(opt) = opt {
                apply_approval_option(app, opt);
            }
        }
        _ => {}
    }
}

/// Apply the chosen approval option. `AcceptAll` flips `auto_apply`
/// as a side effect *before* sending `Verdict::Accept`, so the
/// next tool call observes the new policy. No timeline note - the
/// policy-hint row's label change is the visual confirmation.
fn apply_approval_option(app: &mut AppState, opt: ApprovalOption) {
    match opt {
        ApprovalOption::Accept => send_verdict(app, Verdict::Accept),
        ApprovalOption::AcceptAll => {
            // Only diff prompts expose this option today (see the
            // `approval_options` table). Flip to Safe so subsequent
            // edits auto-apply; shell behavior is unaffected by the
            // mode change. If a shell-side "Accept all" lands
            // later, route it here with kind-specific target mode.
            app.set_auto_apply(AutoApply::Safe);
            tracing::info!(mode = ?AutoApply::Safe, "approval mode set via Accept-all");
            send_verdict(app, Verdict::Accept);
        }
        ApprovalOption::Reject => send_verdict(app, Verdict::Reject),
    }
}

fn submit(app: &mut AppState) {
    // Pre-flight: empty model would fail at the provider with a
    // cryptic 400. Surface it here as a clear note instead. Gate
    // on actual user input first so a bare Enter on an empty
    // buffer stays a no-op.
    if app.mode == AppMode::Idle
        && !app.input.lines().join("\n").trim().is_empty()
        && app.cfg.provider.model.is_empty()
    {
        app.timeline
            .push_note("no model selected - use /model or /settings".into());
        return;
    }
    if let Some(text) = check_and_take_input(app) {
        let handle = spawn_turn(
            text.clone(),
            Arc::clone(&app.agent),
            app.cfg.provider.model.clone(),
            app.agent_tx.clone(),
        );
        app.turn_handle = Some(handle);
        app.history.push(text);
    }
}

/// Validate submit preconditions and consume the input buffer if good.
///
/// Returns `Some(text)` to dispatch when:
/// * the app is currently [`AppMode::Idle`] (no streaming turn), and
/// * the input buffer is non-empty after trimming whitespace.
fn check_and_take_input(app: &mut AppState) -> Option<String> {
    if app.mode != AppMode::Idle {
        return None;
    }
    let text = app.input.lines().join("\n");
    if text.trim().is_empty() {
        return None;
    }
    app.timeline.push_user(text.clone());
    app.reset_input();
    app.mode = AppMode::Streaming;
    Some(text)
}

fn spawn_turn(
    text: String,
    agent: Arc<Mutex<Agent>>,
    model: String,
    tx: UnboundedSender<UiMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut a = agent.lock().await;
        // Sync the agent's captured model with whatever the config
        // currently says. /model and /settings only mutate the
        // Config; without this push, the Agent keeps the build-time
        // value forever.
        a.set_model(model);
        let result = a
            .turn(text, |e| {
                let _ = tx.send(UiMsg::Agent(e));
            })
            .await;
        if let Err(e) = result {
            let _ = tx.send(UiMsg::Note(format!("agent error: {e}")));
        }
    })
}

/// Drop history-browse state when the user has modified the
/// recalled entry. After a divergence, the buffer is conceptually
/// a fresh working draft, not a "still-browsing" view - so Up/Down
/// should fall back to the standard cursor-position rules instead
/// of continuing to rotate through history.
//
// String-equality check is fine: typical input lengths are dozens
// of chars and this runs once per keystroke. Allocating a fresh
// `String` via `join("\n")` is negligible vs the cost of the
// keystroke itself.
fn detach_from_history_if_edited(app: &mut AppState) {
    let Some(idx) = app.history.cursor else { return };
    let Some(entry) = app.history.entries.get(idx) else {
        // Dead index - shouldn't happen, but reset defensively.
        app.history.cursor = None;
        app.history.draft = None;
        return;
    };
    let current = app.input.lines().join("\n");
    if current != *entry {
        app.history.cursor = None;
        // The draft snapshot only makes sense while browsing - the
        // user has clearly committed to editing this content, so
        // there's nothing to restore-on-Down-past-newest.
        app.history.draft = None;
    }
}

/// Walk one step toward older history. If we're not currently
/// browsing, snapshot the current draft so it can be restored.
fn recall_older(app: &mut AppState) {
    if app.history.entries.is_empty() {
        return;
    }
    let new_idx = match app.history.cursor {
        None => {
            // Save current input as draft so Down past newest can
            // restore it.
            let draft = app.input.lines().join("\n");
            app.history.draft = Some(draft);
            app.history.entries.len() - 1
        }
        Some(0) => 0, // already at oldest, stay
        Some(i) => i - 1,
    };
    app.history.cursor = Some(new_idx);
    let entry = app.history.entries[new_idx].clone();
    app.set_input(&entry);
}

/// Walk one step toward newer history. Past the newest entry, restore
/// the draft (or empty input if there wasn't one).
fn recall_newer(app: &mut AppState) {
    let Some(i) = app.history.cursor else {
        return;
    };
    let next = i + 1;
    if next >= app.history.entries.len() {
        // Past the newest -> restore draft / empty.
        app.history.cursor = None;
        let draft = app.history.draft.take().unwrap_or_default();
        if draft.is_empty() {
            app.reset_input();
        } else {
            app.set_input(&draft);
        }
    } else {
        app.history.cursor = Some(next);
        let entry = app.history.entries[next].clone();
        app.set_input(&entry);
    }
}


/// Universal "force-quit" chord. Honored before any modal-specific
/// key handling so the user is never trapped inside a modal.
/// `pub(super)` so sibling modules (slash, model_picker, settings)
/// can guard their own dispatch with the same chord without
/// duplicating the match shape.
pub(super) fn is_ctrl_d(k: KeyEvent) -> bool {
    matches!(
        (k.code, k.modifiers),
        (KeyCode::Char('d'), KeyModifiers::CONTROL)
    )
}

/// Match the Ctrl+C chord. Semantics are context-specific (cancel
/// turn / dismiss modal / chord progression), but the *match* is the
/// same everywhere - this helper just centralizes the pattern so
/// each call site reads as intent, not boilerplate.
pub(super) fn is_ctrl_c(k: KeyEvent) -> bool {
    matches!(
        (k.code, k.modifiers),
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
    )
}

#[cfg(test)]
#[path = "../tests/tui/input.rs"]
mod tests;
