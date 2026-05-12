//! Modal help overlay (shown when `app.show_help` is true).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use super::fg;

/// Horizontal padding (columns) inside the modal border. 2 cols of
/// breathing room on each side keeps the keybind columns from
/// touching the border without burning vertical space.
const H_PAD: u16 = 2;
/// Border thickness on each edge (top, bottom, left, right).
const BORDER: u16 = 1;
/// The titled border (" Keybindings ") consumes a few horizontal
/// cells of the top edge; pad the modal width so the title doesn't
/// crowd the corner glyph.
const TITLE_PAD: u16 = 2;

/// Render the help overlay. Sized to fit the longest keybind row +
/// total content height (plus border/padding), then clamped so we
/// never exceed the frame. No fixed percentage - the modal is as
/// small as the content allows so it doesn't dominate a tall
/// terminal. Centered horizontally and vertically within the frame.
pub(super) fn render_help_overlay(frame: &mut Frame) {
    // Build content first so we can measure it for the modal rect.
    let lines = vec![
        Line::from(Span::styled(
            "Conversation",
            fg(Color::Cyan, Modifier::BOLD | Modifier::DIM),
        )),
        keybind_line("Enter", "Submit message"),
        keybind_line("Shift+Enter / Alt+Enter", "Insert newline"),
        keybind_line("Esc", "Cancel turn / clear (2x) / quit (2x)"),
        keybind_line("Ctrl+C", "Cancel turn / clear input / quit (2x)"),
        keybind_line("Ctrl+D", "Quit"),
        keybind_line("Up / Down", "Recall input history (at edges)"),
        keybind_line("Ctrl+Z", "Undo input edit"),
        keybind_line("Ctrl+Backspace", "Delete previous word"),
        keybind_line("Ctrl+Left / Right", "Word jump within input"),
        keybind_line("PgUp / PgDn", "Scroll conversation"),
        keybind_line("Mouse wheel", "Scroll conversation"),
        keybind_line("Click + drag", "Select text (auto-copies on release by default)"),
        Line::from(""),
        Line::from(Span::styled(
            "Approval prompts",
            fg(Color::Cyan, Modifier::BOLD | Modifier::DIM),
        )),
        keybind_line("Up / Down", "Navigate menu options"),
        keybind_line("Enter", "Confirm highlighted option"),
        keybind_line("y / a / n", "Accept / Accept all / Reject"),
        keybind_line("Esc", "Reject"),
        Line::from(""),
        Line::from(Span::styled(
            "Modes",
            fg(Color::Cyan, Modifier::BOLD | Modifier::DIM),
        )),
        keybind_line("Shift+Tab", "Toggle approval mode (ask edits ↔ auto edits)"),
        keybind_line("/", "Slash commands (on empty input; /help, /settings, /model, /clear, /quit)"),
        Line::from(""),
        // Footer matches the settings-modal footer style: Cyan-
        // dim key glyphs over dim body text, so both overlays
        // read as the same UI family.
        Line::from(vec![
            Span::styled("Esc", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" or ", Style::default().dim()),
            Span::styled("Ctrl+C", fg(Color::Cyan, Modifier::DIM)),
            Span::styled(" to close", Style::default().dim()),
        ]),
    ];

    // Size to content + chrome (border + padding). When the frame is
    // smaller than `desired_*`, we clamp; the Paragraph clips
    // gracefully.
    //
    // Saturating casts: real terminals are never within u16 of
    // overflow, and clamping to MAX still renders something sensible
    // if they were.
    let content_w = u16::try_from(lines.iter().map(Line::width).max().unwrap_or(40))
        .unwrap_or(u16::MAX);
    let content_h = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let desired_w = content_w + 2 * (BORDER + H_PAD) + TITLE_PAD;
    let desired_h = content_h + 2 * BORDER;

    let frame_area = frame.area();
    let width = desired_w.min(frame_area.width);
    let height = desired_h.min(frame_area.height);
    let area = Rect {
        x: frame_area.x + frame_area.width.saturating_sub(width) / 2,
        y: frame_area.y + frame_area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    // `Clear` paints the area with empty cells so the overlay covers
    // whatever was rendered behind it.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Keybindings ")
        .padding(Padding::new(H_PAD, H_PAD, 0, 0));

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn keybind_line(key: &'static str, desc: &'static str) -> Line<'static> {
    // Leading two-space indent matches the settings modal's row
    // gutter, visually grouping keybinds under their section
    // header (same pattern as `Provider` -> `> model    ...`).
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{key:<26}"),
            fg(Color::Cyan, Modifier::BOLD),
        ),
        Span::raw(desc),
    ])
}
