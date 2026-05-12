//! Minimal markdown parser for assistant prose.
//!
//! Subset rationale: real chat output uses ~5% of CommonMark. We parse
//! bold / italic / inline-code / headers (h1-h3, h4-h6 collapse to h3)
//! / bullet lists / numbered lists / horizontal rules and leave the
//! rest as literal text. Constrained subset reads better in a terminal
//! than full CommonMark, which tends to look noisy when every word
//! gets emphasis. Constraints also make the parser easy to understand
//! and small enough to skip a `pulldown-cmark` dependency.
//!
//! Pure data: no ratatui types here. The renderer in
//! [`super::render`] turns [`Block`] / [`Inline`] into styled `Line`s.
//!
//! Streaming-friendly: parsing is per-source-line and stateless across
//! lines. An unclosed emphasis marker in a chunk renders as literal
//! text; the next frame (after the closing marker has arrived in the
//! stream) re-parses the full line and emits emphasis cleanly.
//!
//! # Deliberate non-features
//!
//! Links (`[text](url)`), tables, blockquotes, HTML, strikethrough,
//! backslash escapes, and nested-list indentation are all out of
//! scope. The model uses them rarely in chat replies; adding them
//! means much more parser complexity for marginal display value.

/// Block-level shape of a single source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `# Title` through `### Title`. H4+ collapse to level 3 - deep
    /// heading hierarchies don't pay their visual weight in chat.
    Heading { level: u8, inline: Vec<Inline> },
    /// `- item`, `* item`, or `+ item` at start of line.
    Bullet(Vec<Inline>),
    /// `1. item`, `42. item`, etc. The original number is preserved
    /// so consecutive items keep the user's numbering.
    Numbered { number: u32, inline: Vec<Inline> },
    /// Standalone `---`, `***`, or `___` (3+ identical markers,
    /// optionally space-separated) on its own line.
    Rule,
    /// Anything else: regular paragraph content.
    Paragraph(Vec<Inline>),
}

/// Inline-level token within a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// `**foo**` or `__foo__`. Nests further inline markup.
    Bold(Vec<Inline>),
    /// `*foo*` or `_foo_`. Nests further inline markup.
    Italic(Vec<Inline>),
    /// Single-backtick `` `foo` ``. Verbatim; no nested parsing.
    Code(String),
}

/// Parse one whole line of markdown into a `Block`. The caller splits
/// the source on `\n` and feeds lines one at a time.
#[must_use]
pub fn parse_line(line: &str) -> Block {
    let trimmed = line.trim_start();

    if is_horizontal_rule(trimmed) {
        return Block::Rule;
    }
    if let Some((level, rest)) = parse_heading_prefix(trimmed) {
        // H4-H6 collapse to H3 (deep nesting doesn't carry).
        return Block::Heading {
            level: level.min(3),
            inline: parse_inline(rest),
        };
    }
    if let Some(rest) = parse_bullet_prefix(trimmed) {
        return Block::Bullet(parse_inline(rest));
    }
    if let Some((n, rest)) = parse_numbered_prefix(trimmed) {
        return Block::Numbered {
            number: n,
            inline: parse_inline(rest),
        };
    }
    // Default - parse the original line (preserving any leading
    // whitespace the source had) as paragraph inline content.
    Block::Paragraph(parse_inline(line))
}

/// 3+ of `-` / `*` / `_`, optionally space-separated, nothing else.
fn is_horizontal_rule(s: &str) -> bool {
    let s = s.trim();
    let first = match s.chars().next() {
        Some(c) if c == '-' || c == '*' || c == '_' => c,
        _ => return false,
    };
    let count = s.chars().filter(|c| *c == first).count();
    count >= 3 && s.chars().all(|c| c == first || c == ' ')
}

/// Recognize `# `, `## `, ... up to `###### ` and return the level
/// (1-6) and the heading text.
fn parse_heading_prefix(s: &str) -> Option<(u8, &str)> {
    let level = s.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &s[level..];
    // ATX headings require a space between hashes and content.
    if !rest.starts_with(' ') {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some((level as u8, rest.trim_start()))
}

/// Recognize `- `, `* `, `+ ` (bullet markers must be followed by a space).
fn parse_bullet_prefix(s: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(marker) {
            return Some(rest);
        }
    }
    None
}

/// Recognize `N. ` (one or more digits, dot, space).
fn parse_numbered_prefix(s: &str) -> Option<(u32, &str)> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &s[digits.len()..];
    let rest = after.strip_prefix(". ")?;
    let n: u32 = digits.parse().ok()?;
    Some((n, rest))
}

/// Parse inline markup within a single text run. Emphasis markers
/// without a matching close render as literal characters; the
/// parser never errors out.
#[must_use]
pub fn parse_inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    parse_inline_slice(&chars)
}

fn parse_inline_slice(chars: &[char]) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < chars.len() {
        // `inline code`: single backtick, verbatim until matching backtick.
        if chars[i] == '`'
            && let Some(end) = find_closing_backtick(chars, i + 1)
        {
            flush_text(&mut buf, &mut out);
            out.push(Inline::Code(chars[i + 1..end].iter().collect()));
            i = end + 1;
            continue;
        }
        // **bold** or __bold__
        if is_double_open(chars, i)
            && let Some(end) = find_double_close(chars, i + 2, chars[i])
        {
            flush_text(&mut buf, &mut out);
            let inner = parse_inline_slice(&chars[i + 2..end]);
            out.push(Inline::Bold(inner));
            i = end + 2;
            continue;
        }
        // *italic* or _italic_
        if is_single_open(chars, i)
            && let Some(end) = find_single_close(chars, i + 1, chars[i])
        {
            flush_text(&mut buf, &mut out);
            let inner = parse_inline_slice(&chars[i + 1..end]);
            out.push(Inline::Italic(inner));
            i = end + 1;
            continue;
        }
        buf.push(chars[i]);
        i += 1;
    }
    flush_text(&mut buf, &mut out);
    out
}

fn flush_text(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Text(std::mem::take(buf)));
    }
}

fn find_closing_backtick(chars: &[char], start: usize) -> Option<usize> {
    (start..chars.len()).find(|&i| chars[i] == '`')
}

/// Position `i` opens a `**` / `__` pair: the marker is doubled and
/// the character after the pair is non-whitespace (CommonMark's
/// flanking-delimiter rule, simplified).
fn is_double_open(chars: &[char], i: usize) -> bool {
    let c = chars[i];
    (c == '*' || c == '_')
        && chars.get(i + 1) == Some(&c)
        && chars.get(i + 2).is_some_and(|n| !n.is_whitespace())
}

/// Position `i` opens a single `*` / `_` italic: the marker is *not*
/// doubled (otherwise it'd be bold) and the next char is non-ws.
fn is_single_open(chars: &[char], i: usize) -> bool {
    let c = chars[i];
    (c == '*' || c == '_')
        && chars.get(i + 1).is_some_and(|n| *n != c && !n.is_whitespace())
}

/// Find the close `**` for an open at `start-2`. Requires
/// non-whitespace immediately *before* the close, also per the
/// flanking-delimiter rule.
fn find_double_close(chars: &[char], start: usize, marker: char) -> Option<usize> {
    let mut i = start;
    while i + 1 < chars.len() {
        if chars[i] == marker
            && chars[i + 1] == marker
            && i > start
            && !chars[i - 1].is_whitespace()
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find the close `*` / `_` for a single-marker italic open. Avoids
/// matching `**` (a bold close) by checking the next char isn't the
/// same marker.
fn find_single_close(chars: &[char], start: usize, marker: char) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == marker
            && i > start
            && !chars[i - 1].is_whitespace()
            && chars.get(i + 1) != Some(&marker)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
#[path = "../tests/tui/markdown.rs"]
mod tests;
