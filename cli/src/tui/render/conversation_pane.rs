//! Conversation pane rendering: chat history, tool calls, notes,
//! and the trailing streaming / approval-body indicator.
//!
//! The pane walks [`Timeline`](super::super::timeline::Timeline) once
//! per frame and emits styled [`Line`]s for user messages, assistant
//! text (with markdown + triple-backtick code-fence detection), tool
//! calls (running / done / error), and notes. Diff bodies for pending
//! approvals are appended inline below the conversation - the
//! approval pin holds only the header + menu.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};

use super::super::app::{AppMode, AppState};
use super::super::markdown;
use super::super::timeline::{TimelineItem, ToolStatus};
use super::layout::{CONV_PAD_BOTTOM, CONV_PAD_TOP, PANE_HORIZONTAL_PAD};
use super::{fg, format_duration, no_color, sanitize_for_display};

/// Spinner frames used while streaming. Braille phases give a smooth
/// rotation in modern terminals; older terminals fall back gracefully
/// to the substitute glyphs.
pub const SPINNER_FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
];

/// Which speaker a timeline item belongs to. Used by the role-transition
/// header logic in [`render_conversation`] so the assistant's "Lumen"
/// label is emitted exactly once per turn - even when the turn opens
/// with a tool call rather than text. Notes return `None` (they're
/// meta and don't change role).
#[derive(Copy, Clone, PartialEq, Eq)]
enum Speaker {
    User,
    Lumen,
}

fn speaker_of(item: &TimelineItem) -> Option<Speaker> {
    match item {
        TimelineItem::User(_) => Some(Speaker::User),
        TimelineItem::AssistantText(_) | TimelineItem::ToolCall { .. } => Some(Speaker::Lumen),
        TimelineItem::Note(_) => None,
    }
}

fn user_header() -> Line<'static> {
    Line::from(Span::styled("You", fg(Color::White, Modifier::BOLD)))
}

fn lumen_header() -> Line<'static> {
    // Bold cyan rather than `LightBlue`: the latter (ANSI 12) gets
    // remapped to purple/pink by many terminal themes (Solarized,
    // various dark themes), while ANSI 6 cyan renders consistently.
    Line::from(Span::styled("Lumen", fg(Color::Cyan, Modifier::BOLD)))
}

pub(super) fn render_conversation(frame: &mut Frame, area: Rect, app: &mut AppState) {
    let block = Block::new().padding(Padding::new(
        PANE_HORIZONTAL_PAD,
        PANE_HORIZONTAL_PAD,
        CONV_PAD_TOP,
        CONV_PAD_BOTTOM,
    ));

    if app.timeline.is_empty()
        && !matches!(app.mode, AppMode::Streaming)
        && app.pending_approval.is_none()
    {
        let p = Paragraph::new("Type a message to begin. Run /help for keybindings.")
            .style(Style::default().dim())
            .block(block);
        frame.render_widget(p, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut last_speaker: Option<Speaker> = None;
    let mut first_block = true;

    for item in app.timeline.items() {
        // Blank line above every block except the very first. Gives a
        // consistent rhythm: user blocks, assistant blocks, tool calls,
        // and the streaming indicator are all separated by exactly one
        // blank row.
        if !first_block {
            lines.push(Line::default());
        }
        first_block = false;

        // Emit a speaker header on role transitions. Notes return None
        // and don't change last_speaker, so a Note in the middle of an
        // assistant turn doesn't trigger a duplicate "Lumen" header on
        // the next assistant block.
        if let Some(speaker) = speaker_of(item)
            && last_speaker != Some(speaker)
        {
            lines.push(match speaker {
                Speaker::User => user_header(),
                Speaker::Lumen => lumen_header(),
            });
            last_speaker = Some(speaker);
        }

        match item {
            TimelineItem::User(text) => render_text_content(&mut lines, text, false),
            TimelineItem::AssistantText(text) => render_text_content(&mut lines, text, true),
            TimelineItem::ToolCall {
                name,
                arguments,
                status,
                elapsed,
                ..
            } => render_tool_call(&mut lines, name, arguments, status, *elapsed, &app.cwd),
            TimelineItem::Note(text) => render_note(&mut lines, text),
        }
    }

    append_trailing_indicator(&mut lines, app, first_block);

    let para = Paragraph::new(lines).wrap(Wrap { trim: false }).block(block);

    // Scroll math: `line_count` and the visible-row math both operate
    // on the block's *inner* area, which is the outer area minus the
    // padding we declared above.
    let inner_width = area.width.saturating_sub(PANE_HORIZONTAL_PAD * 2);
    let inner_height = area
        .height
        .saturating_sub(CONV_PAD_TOP + CONV_PAD_BOTTOM);
    let total = para.line_count(inner_width);
    let visible = inner_height as usize;
    let bottom_anchor = total.saturating_sub(visible);
    if app.scroll_offset > bottom_anchor {
        app.scroll_offset = bottom_anchor;
    }

    // Translate any active selection by the *post-clamp* scroll delta.
    // Doing this here (rather than in the input handlers) means a
    // wheel-up at the very top - which the clamp pins back to
    // bottom_anchor - doesn't leak a phantom shift into the selection.
    if app.scroll_offset != app.render.last_rendered_scroll
        && let Some(sel) = app.selection.as_mut()
    {
        if app.scroll_offset > app.render.last_rendered_scroll {
            let d = u16::try_from(app.scroll_offset - app.render.last_rendered_scroll)
                .unwrap_or(u16::MAX);
            sel.anchor.1 = sel.anchor.1.saturating_add(d);
            sel.focus.1 = sel.focus.1.saturating_add(d);
        } else {
            let d = u16::try_from(app.render.last_rendered_scroll - app.scroll_offset)
                .unwrap_or(u16::MAX);
            sel.anchor.1 = sel.anchor.1.saturating_sub(d);
            sel.focus.1 = sel.focus.1.saturating_sub(d);
        }
    }
    app.render.last_rendered_scroll = app.scroll_offset;

    let scroll = bottom_anchor.saturating_sub(app.scroll_offset);
    let scroll_y: u16 = scroll.try_into().unwrap_or(u16::MAX);
    frame.render_widget(para.scroll((scroll_y, 0)), area);
}

/// Render free-form text content (user input or assistant prose) into
/// styled [`Line`]s with our shared blank-line discipline:
///
/// * **Sanitized**: every input line is run through [`sanitize_for_display`]
///   first, replacing tabs and other control characters with spaces.
///   Without this, a model that emits raw ANSI escapes (deliberately or
///   via prompt injection) corrupts the terminal render the same way
///   the read-tool's `\t` did before we caught it.
/// * **Trailing blanks dropped**: `lines()` strips a single trailing
///   `\n`, and we additionally buffer blank lines and only flush them
///   when the next non-blank line appears - so trailing blank source
///   lines vanish (the inter-block separator handles the gap to the
///   next timeline block).
/// * **Internal blanks preserved**: paragraph breaks (`\n\n`) inside
///   the block render as a single blank row, matching the model's intent.
/// * **Markdown** (when `render_markdown = true`):
///     * triple-backtick fences toggle a yellow-content code-block
///       state (fence-marker lines are eaten);
///     * non-code lines are run through [`super::super::markdown::parse_line`]
///       for headers / lists / bold / italic / inline code rendering;
///     * single-backtick inline code shares the same yellow as fenced
///       code so "code, not prose" has one visual signal.
//
// Blank source lines push `Line::default()` (truly empty) rather than
// `Line::from("  ")` (whitespace content) - ratatui's `Wrap { trim:
// false }` renders the latter as TWO visual rows instead of one, AND
// only paints columns 0..2, leaving stale buffer cells past column 1.
// The visible artifacts are doubled blank lines and ghost characters
// at the right side of the conversation pane after scroll. Empty Lines
// avoid both issues; ratatui paints the full row with the area's style.
fn render_text_content(out: &mut Vec<Line<'static>>, text: &str, render_markdown: bool) {
    let mut in_code = false;
    let mut pending_blanks = 0usize;
    for raw_line in text.lines() {
        let line = sanitize_for_display(raw_line);
        if render_markdown && line.trim_start().starts_with("```") {
            in_code = !in_code;
            // Treat fence markers as if no blank crossed them; otherwise
            // a blank line right before a closing fence would survive.
            pending_blanks = 0;
            continue;
        }
        // `trim().is_empty()` (rather than `is_empty()`) treats lines
        // that were originally pure control chars as blank, since
        // sanitize replaced them with spaces.
        if line.trim().is_empty() {
            pending_blanks += 1;
            continue;
        }
        for _ in 0..pending_blanks {
            out.push(Line::default());
        }
        pending_blanks = 0;
        out.push(format_text_line(&line, render_markdown, in_code));
    }
    // Trailing blanks (`pending_blanks > 0` after the loop) are dropped.
}

/// Convert one sanitized, non-blank source line into a styled `Line`.
/// Three paths:
///   * inside a fenced code block -> whole line yellow, no inline parse
///   * markdown mode (outside fences) -> parse block + inline, style
///   * plain mode -> indent + literal text
fn format_text_line(line: &str, render_markdown: bool, in_fence: bool) -> Line<'static> {
    if in_fence {
        return Line::from(format!("  {line}"))
            .style(fg(Color::LightYellow, Modifier::empty()));
    }
    if !render_markdown {
        return Line::from(format!("  {line}"));
    }
    render_markdown_block(markdown::parse_line(line))
}

/// Render one parsed [`markdown::Block`] into a styled `Line`. Each
/// block kind has its own marker + base style; inline tokens within
/// the block flow through [`render_inline`].
fn render_markdown_block(block: markdown::Block) -> Line<'static> {
    /// Two-space body indent matches the input pane's prompt gutter and
    /// the plain-text rendering path - all assistant content lines up.
    const INDENT: &str = "  ";
    match block {
        markdown::Block::Heading { level, inline } => {
            // h1 = bold cyan, h2 = plain bold, h3 = bold + dim. Smaller
            // headings feel quieter; deep hierarchies in chat output
            // were collapsed to h3 by the parser already.
            let style = match level {
                1 => fg(Color::Cyan, Modifier::BOLD),
                2 => Style::new().add_modifier(Modifier::BOLD),
                _ => Style::new().add_modifier(Modifier::BOLD | Modifier::DIM),
            };
            let mut spans = vec![Span::raw(INDENT)];
            spans.extend(render_inline(&inline, style));
            Line::from(spans)
        }
        markdown::Block::Bullet(inline) => {
            let mut spans = vec![
                Span::raw(INDENT),
                Span::styled("• ", fg(Color::Cyan, Modifier::DIM)),
            ];
            spans.extend(render_inline(&inline, Style::default()));
            Line::from(spans)
        }
        markdown::Block::Numbered { number, inline } => {
            let mut spans = vec![
                Span::raw(INDENT),
                Span::styled(format!("{number}. "), Style::new().add_modifier(Modifier::DIM)),
            ];
            spans.extend(render_inline(&inline, Style::default()));
            Line::from(spans)
        }
        markdown::Block::Rule => {
            // 60 cells of dim ─ is enough to read as a divider in any
            // reasonable terminal width without doing a runtime layout
            // query for the actual inner width.
            Line::from(Span::styled(
                format!("{INDENT}{}", "─".repeat(60)),
                Style::new().add_modifier(Modifier::DIM),
            ))
        }
        markdown::Block::Paragraph(inline) => {
            let mut spans = vec![Span::raw(INDENT)];
            spans.extend(render_inline(&inline, Style::default()));
            Line::from(spans)
        }
    }
}

/// Walk inline tokens emitting styled spans. Emphasis composes by
/// layering modifiers on top of `base`; inline code overrides the
/// foreground color (or falls back to `DIM` under NO_COLOR) while
/// preserving any modifiers the enclosing emphasis added.
fn render_inline(tokens: &[markdown::Inline], base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(tokens.len());
    for token in tokens {
        match token {
            markdown::Inline::Text(s) => spans.push(Span::styled(s.clone(), base)),
            markdown::Inline::Bold(inner) => {
                spans.extend(render_inline(inner, base.add_modifier(Modifier::BOLD)));
            }
            markdown::Inline::Italic(inner) => {
                spans.extend(render_inline(inner, base.add_modifier(Modifier::ITALIC)));
            }
            markdown::Inline::Code(s) => {
                // Inline `code` shares the fenced-block yellow so the
                // visual signal for "this is code, not prose" is one
                // colour, not two.
                let style = if no_color() {
                    base.add_modifier(Modifier::DIM)
                } else {
                    base.fg(Color::LightYellow)
                };
                spans.push(Span::styled(s.clone(), style));
            }
        }
    }
    spans
}

fn render_tool_call(
    out: &mut Vec<Line<'static>>,
    name: &str,
    arguments: &str,
    status: &ToolStatus,
    elapsed: Option<Duration>,
    cwd: &std::path::Path,
) {
    // Visual severity: a user rejection lives in `Done(result)`
    // protocol-wise (the tool returned Ok with a "REJECTED ..."
    // message so the model gets a clean string), but it isn't a
    // success - render it the same way an error renders so the
    // user can tell at a glance that the operation didn't apply.
    // Detection is by the [`lumen_core::REJECTION_PREFIX`] const
    // shared with the tools that produce it, so the strings can't
    // drift.
    let is_rejected = matches!(
        status,
        ToolStatus::Done(result) if result.starts_with(lumen_core::REJECTION_PREFIX)
    );
    let icon_color = match status {
        ToolStatus::Running => Color::Yellow,
        ToolStatus::Done(_) if is_rejected => Color::Red,
        ToolStatus::Done(_) => Color::Green,
        ToolStatus::Error(_) => Color::Red,
    };
    let body_style = if is_rejected || matches!(status, ToolStatus::Error(_)) {
        fg(Color::Red, Modifier::DIM)
    } else {
        Style::new().add_modifier(Modifier::DIM)
    };

    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("● ", fg(icon_color, Modifier::empty())),
        Span::styled(name.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw("("),
        Span::styled(short_args_with_paths(name, arguments, cwd), Style::new().dim()),
        Span::raw(")"),
    ]));

    let timing = elapsed.map(format_duration).unwrap_or_default();
    let body = match status {
        ToolStatus::Running => String::from("    ⎿ running…"),
        ToolStatus::Done(result) => {
            // No "error:" prefix on rejection - the result text
            // already leads with "REJECTED by user:" which is the
            // load-bearing signal for both the model and the user.
            if timing.is_empty() {
                format!("    ⎿ {}", preview(result))
            } else {
                format!("    ⎿ {} ({timing})", preview(result))
            }
        }
        ToolStatus::Error(err) => {
            if timing.is_empty() {
                format!("    ⎿ error: {}", preview(err))
            } else {
                format!("    ⎿ error: {} ({timing})", preview(err))
            }
        }
    };
    out.push(Line::from(body).style(body_style));
}

fn render_note(out: &mut Vec<Line<'static>>, text: &str) {
    // Two visual tiers: informational notes stay dim with a `·`
    // bullet ("Cooked for 4.2s", "cancelled by user"); errors get
    // a red `✗` marker so they pop against the dim conversation
    // padding. Classification is heuristic at the renderer level
    // since the underlying `Note(String)` doesn't carry kind data
    // yet - extending to `Note { text, kind }` is forward-work
    // when more note kinds accrue.
    let is_error = note_is_error(text);
    let (marker, style) = if is_error {
        ("✗ ", fg(Color::Red, Modifier::BOLD))
    } else {
        ("· ", Style::new().add_modifier(Modifier::DIM))
    };
    out.push(Line::from(vec![
        Span::styled(marker, style),
        Span::styled(text.to_string(), style),
    ]));
}

/// Notes coming out of the agent / transport layer use these
/// prefixes; the renderer treats them as error-tier. Match is
/// case-sensitive because the producers (`spawn_turn` in input.rs,
/// `Session::push` failures) emit lowercase.
fn note_is_error(text: &str) -> bool {
    const ERROR_PREFIXES: &[&str] = &["agent error:", "transport error:"];
    ERROR_PREFIXES.iter().any(|p| text.starts_with(p))
        || text.contains(" failed:")
        || text.contains(" failed ")
}

/// Append either the approval body (for diff prompts) or the
/// streaming spinner (when a turn is in flight without an approval
/// pending) to the conversation-pane lines.
//
// Approval body takes precedence: while a tool awaits a verdict the
// model isn't generating, so "Lumen is thinking…" would mislead. The
// pin's menu is the visual signal that we're awaiting input.
//
// Streaming spinner: label adapts to the active tool, fetched from
// the trailing `Running` ToolCall via `active_tool_message`. Falls
// back to "Lumen is thinking…" between tool calls.
fn append_trailing_indicator(lines: &mut Vec<Line<'static>>, app: &AppState, first_block: bool) {
    if let Some(pending) = &app.pending_approval {
        let body = super::approval_panel::approval_body_lines(pending, &app.cwd);
        if !body.is_empty() {
            if !first_block {
                lines.push(Line::default());
            }
            lines.extend(body);
        }
        return;
    }
    if !matches!(app.mode, AppMode::Streaming) {
        return;
    }
    if !first_block {
        lines.push(Line::default());
    }
    let spinner_frame = SPINNER_FRAMES[app.render.spinner_tick % SPINNER_FRAMES.len()];
    let label = active_tool_message(app).unwrap_or_else(|| "Lumen is thinking…".to_string());
    lines.push(Line::from(vec![
        Span::styled(spinner_frame.to_string(), fg(Color::Cyan, Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(label, Style::default().add_modifier(Modifier::DIM)),
    ]));
}

/// Build the dynamic streaming-spinner label from `AppState`'s
/// explicit `active_tool` field. Returns `None` when no tool is
/// currently "in flight from the user's perspective" - caller
/// falls back to the generic "Lumen is thinking…".
//
// History: this used to reverse-scan the timeline for a `Running`
// ToolCall, which silently broke for fast tools. The event-loop's
// drain-then-render pattern collapses `ToolCallStart` +
// `ToolCallEnd` into a single render tick for tools that complete
// in microseconds (local file reads/writes/edits), so no frame
// ever paints with `Running` status. The explicit field, set on
// Start and kept across End, guarantees at least one frame with
// the dynamic label even for instant tools.
fn active_tool_message(app: &AppState) -> Option<String> {
    let (name, args) = app.active_tool.as_ref()?;
    Some(format_tool_action(name, args, &app.cwd))
}

/// Map `(tool_name, args)` to a human-readable progress label.
/// Parses the args JSON best-effort; falls back to a generic verb
/// when a field is missing or the args aren't JSON. Paths are
/// shortened via [`super::super::app::display_path`] so cwd-rooted
/// ops show as `./file` instead of the full absolute form.
pub fn format_tool_action(name: &str, args: &str, cwd: &std::path::Path) -> String {
    let val: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
    let path_of = |key: &str| -> String {
        val.get(key)
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || String::from("file"),
                |s| super::super::app::display_path(std::path::Path::new(s), cwd),
            )
    };
    match name {
        "read" => format!("Reading {}…", path_of("path")),
        "write" => format!("Writing {}…", path_of("path")),
        "edit" => format!("Editing {}…", path_of("path")),
        "grep" => {
            let pat = val
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if pat.is_empty() {
                "Searching…".to_string()
            } else {
                format!("Searching for {pat}…")
            }
        }
        "shell" => {
            let cmd = val
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if cmd.is_empty() {
                "Running command…".to_string()
            } else {
                let mut display = cmd.to_string();
                if display.len() > 40 {
                    let mut end = 40;
                    while !display.is_char_boundary(end) {
                        end -= 1;
                    }
                    display.truncate(end);
                    display.push('…');
                }
                format!("Running: {display}")
            }
        }
        other => format!("Running {other}…"),
    }
}

/// Path-aware variant of [`short_args`] that rewrites the `path`
/// field of file-bearing tools to cwd-relative form before
/// truncation. Falls back to raw [`short_args`] on parse failure
/// or for tools without a known path field.
fn short_args_with_paths(name: &str, args: &str, cwd: &std::path::Path) -> String {
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(args) else {
        return short_args(args);
    };
    let path_fields: &[&str] = match name {
        "read" | "write" | "edit" | "grep" => &["path"],
        _ => &[],
    };
    if path_fields.is_empty() {
        return short_args(args);
    }
    if let Some(obj) = val.as_object_mut() {
        for field in path_fields {
            if let Some(serde_json::Value::String(s)) = obj.get_mut(*field) {
                *s = super::super::app::display_path(std::path::Path::new(s), cwd);
            }
        }
    }
    short_args(&val.to_string())
}

/// Truncate `args` to ~60 chars on a UTF-8 boundary, with an ellipsis
/// when shortened. Sanitizes control chars first - a tab in the args
/// would desync ratatui's cell positions from the terminal's.
pub fn short_args(args: &str) -> String {
    let cleaned = sanitize_for_display(args.trim());
    if cleaned.len() <= 60 {
        return cleaned;
    }
    let mut end = 60.min(cleaned.len());
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &cleaned[..end])
}

/// Pick a representative line from a tool result for one-line
/// preview rendering. "Interesting" means: not blank, and not one
/// of the status / header markers that the shell tool prepends
/// (`exit:`, `--- stdout ---`, `--- stderr ---`). Falls back to
/// the first non-blank line if everything looks like a marker.
/// Suffix `(+N more lines)` counts lines beyond the displayed one.
pub fn preview(text: &str) -> String {
    let total = text.lines().count();
    let extra = total.saturating_sub(1);

    // Skip the well-known shell-tool framing so the preview lands
    // on actual output (`hello world`) rather than `exit: 0`.
    let interesting = text
        .lines()
        .find(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("exit:") && !t.starts_with("---")
        })
        .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    let first = sanitize_for_display(interesting);

    if extra > 0 {
        format!("{first} (+{extra} more lines)")
    } else {
        first
    }
}
