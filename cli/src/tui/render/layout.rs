//! Layout math: pane dimensions, wrap utilities, geometry helpers.
//!
//! Houses the constants every render submodule reaches for
//! (`PANE_HORIZONTAL_PAD`, `MAX_INPUT_ROWS`, ...) and the wrap-aware
//! cursor math the input-pane rendering and the input-event handler
//! share. Keeping these together prevents wrap-policy drift between
//! the renderer and the keystroke side - both bottom out here.


use super::super::app::AppState;

/// Minimum rows of conversation pane to keep visible no matter how
/// large the input grows. 5 rows is enough to see ~2 message blocks
/// or a tool call + its result.
pub(super) const MIN_CONVERSATION_ROWS: u16 = 5;

/// Rows of fixed-height widgets pinned BELOW the input pane: the
/// bottom status bar and the visual breathing padding under it.
/// Used by `input_height` to size the input pane honestly without
/// stepping on the conversation's minimum.
pub(super) const BOTTOM_FIXED_ROWS: u16 = 2;

/// Hard ceiling on the input pane height regardless of terminal size.
/// 30 total rows = 2 borders + 28 content rows. Generous enough for
/// substantial code pastes while still preventing the input from
/// dominating very tall terminals. Beyond this the internal scroll
/// in `render_input` covers the overflow case.
pub const MAX_INPUT_ROWS: u16 = 30;

/// Horizontal block-padding shared by the conversation pane and the
/// input pane so their inner content edges line up exactly.
///
/// With this + the conversation's `"  "` body-line prefix + the input
/// pane's 2-col prompt gutter, both panes' content lands at column
/// `PANE_HORIZONTAL_PAD + 2` from the outer frame edge.
pub const PANE_HORIZONTAL_PAD: u16 = 2;

/// Conversation pane vertical padding: 1 row top + bottom for breathing
/// room. The bottom row keeps the latest content (and the streaming
/// indicator) from sitting flush against the input pane's top border.
pub(super) const CONV_PAD_TOP: u16 = 1;
pub(super) const CONV_PAD_BOTTOM: u16 = 1;

/// Block padding (each side) inside the input pane's surrounding block.
pub(super) const INPUT_BLOCK_HPAD: u16 = 2;
/// Width of the prompt gutter ("> ") on the first row of the textarea.
pub(super) const PROMPT_GUTTER_WIDTH: u16 = 2;
/// Combined overhead subtracted from frame width to get the textarea's
/// effective character-wrap width.
const INPUT_INNER_OVERHEAD: u16 = INPUT_BLOCK_HPAD * 2 + PROMPT_GUTTER_WIDTH;

/// Compute the input pane height: two rows of border (TOP + BOTTOM)
/// plus the textarea's current **visual** row count (after wrapping
/// to `frame_width`), clamped against the available frame height so
/// the conversation pane keeps at least [`MIN_CONVERSATION_ROWS`].
//
// Floor of 3 (top border + 1 content row + bottom border) keeps
// the empty-input shape compact, like a normal terminal prompt.
// Multi-line input AND wrapped long lines both grow the pane
// organically - the math goes through `visual_row_count` so the
// height tracks what the user actually sees rendered.
pub fn input_height(app: &AppState, frame_h: u16, frame_w: u16, pin_height: u16) -> u16 {
    let textarea_width = usize::from(textarea_inner_width(frame_w));
    let lines = app.input.lines();
    let content_rows = visual_row_count(lines, textarea_width);

    // Also accommodate the cursor's visual row. Critical for the
    // case where a logical line ends exactly at the wrap boundary:
    // a line of length-multiple-of-width fits in N content rows,
    // but the cursor sits at the start of the (non-existent) N+1th
    // row. Without this max, the pane would shrink the moment a
    // backspace brings the line down to an exact-width count, even
    // though the cursor is still visually "below" the content.
    let (cur_row, cur_col) = lines_cursor(app);
    let (vrow, _) = cursor_to_visual(lines, cur_row, cur_col, textarea_width);
    let cursor_extent = usize::from(vrow).saturating_add(1);

    let visual_rows = content_rows.max(cursor_extent);
    let visual_u16: u16 = u16::try_from(visual_rows).unwrap_or(u16::MAX);

    // Cap that reflects every fixed row in the vertical layout
    // BELOW the input pane (status bar + bottom padding) plus the
    // approval pin ABOVE it, plus `MIN_CONVERSATION_ROWS` for the
    // conversation. Without this, the input was 2+ rows too
    // generous and content kept growing past the pane bottom -
    // the renderer's scroll path picks up the slack now, but the
    // honest cap is what keeps the conversation pane visible.
    let available = frame_h
        .saturating_sub(BOTTOM_FIXED_ROWS)
        .saturating_sub(pin_height);
    // The cap is the tighter of:
    //   * "rooms left after every other fixed region above and
    //     below the input is satisfied" - prevents the input from
    //     squeezing the conversation below its minimum.
    //   * `MAX_INPUT_ROWS` - prevents huge terminals from giving
    //     the input pane a pointlessly tall column. Internal
    //     scroll handles overflow above this ceiling.
    let upper = available
        .saturating_sub(MIN_CONVERSATION_ROWS)
        .clamp(3, MAX_INPUT_ROWS);
    visual_u16.saturating_add(2).clamp(3, upper)
}

/// Local helper so the cursor read happens through one named site
/// (easier to grep / test-mock later if we ever virtualize the
/// textarea backend).
fn lines_cursor(app: &AppState) -> (usize, usize) {
    app.input.cursor()
}

/// Effective character width of the textarea sub-area inside the
/// input pane. Frame width minus:
///   * 4 cols for the input-block's `Padding::horizontal(2)` (2
///     on each side - the block has no LEFT/RIGHT borders, just
///     the padding inside its full-width TOP/BOTTOM borders),
///   * 2 cols for the prompt gutter (`> `).
//
// Built from named constants so a future tweak to padding or the
// prompt gutter updates the wrap math in lockstep with the actual
// render in `render_input`. Saturating subtraction so a
// pathologically narrow frame doesn't underflow.
pub fn textarea_inner_width(frame_width: u16) -> u16 {
    frame_width.saturating_sub(INPUT_INNER_OVERHEAD)
}

/// Total visual rows the textarea will consume when wrapped to
/// `width` cells. Empty logical lines still count as one visual
/// row (the cursor parks there); non-empty lines wrap at every
/// `width` characters.
pub fn visual_row_count(lines: &[String], width: usize) -> usize {
    if width == 0 {
        // Pathological - just count logical lines so we don't
        // divide by zero.
        return lines.len().max(1);
    }
    lines
        .iter()
        .map(|line| {
            let len = line.chars().count();
            if len == 0 {
                1
            } else {
                len.div_ceil(width)
            }
        })
        .sum::<usize>()
        .max(1)
}

/// Character-wrap one logical line into a sequence of visual lines,
/// each at most `width` characters wide. Char-boundary safe
/// (chars(), not bytes). Empty input -> a single empty visual line
/// so the cursor has a row to land on.
pub fn char_wrap(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![line.to_string()];
    }
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut col = 0;
    for c in line.chars() {
        current.push(c);
        col += 1;
        if col >= width {
            out.push(std::mem::take(&mut current));
            col = 0;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Map a `(logical_row, logical_col)` cursor position to the
/// matching `(visual_row, visual_col)` in the wrapped view. Same
/// math `char_wrap` uses, so the cursor lands on the right cell.
pub fn cursor_to_visual(
    lines: &[String],
    cur_row: usize,
    cur_col: usize,
    width: usize,
) -> (u16, u16) {
    if width == 0 {
        let r = u16::try_from(cur_row).unwrap_or(u16::MAX);
        return (r, 0);
    }
    let mut vrow = 0usize;
    for line in lines.iter().take(cur_row) {
        let len = line.chars().count();
        vrow += if len == 0 { 1 } else { len.div_ceil(width) };
    }
    vrow += cur_col / width;
    let vcol = cur_col % width;
    (
        u16::try_from(vrow).unwrap_or(u16::MAX),
        u16::try_from(vcol).unwrap_or(u16::MAX),
    )
}

/// Inverse of [`cursor_to_visual`]: given an absolute visual row +
/// column under the same wrap policy, return the matching
/// `(logical_row, logical_col)`. The target column is clamped to
/// the matched logical line's char count so navigating to a
/// shorter wrap row lands at the line's end rather than off the
/// edge. Used by wrap-aware Up/Down cursor movement to preserve
/// visual column across visual rows.
//
// `width == 0` is treated as "no wrap" - returns the trivial
// mapping `(0, vcol_target)` so test fixtures without a known
// pane width fall back to logical-row semantics rather than
// panicking on a zero-width modulo.
pub fn visual_to_logical(
    lines: &[String],
    vrow_target: usize,
    vcol_target: usize,
    width: usize,
) -> (usize, usize) {
    if width == 0 || lines.is_empty() {
        return (0, vcol_target);
    }
    let mut vrow_seen = 0usize;
    for (lr, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        let subrows = if len == 0 { 1 } else { len.div_ceil(width) };
        if vrow_target < vrow_seen + subrows {
            let subrow_in_line = vrow_target - vrow_seen;
            let target_col = (subrow_in_line * width + vcol_target).min(len);
            return (lr, target_col);
        }
        vrow_seen += subrows;
    }
    // Past the last visual row - clamp to end-of-buffer.
    let last_row = lines.len() - 1;
    let last_len = lines.last().map_or(0, |l| l.chars().count());
    (last_row, last_len)
}

