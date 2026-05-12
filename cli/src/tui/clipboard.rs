//! System-clipboard write via OSC 52 escape sequences.
//!
//! The TUI's mouse capture intercepts click+drag, which would normally
//! drive the terminal's native selection. We re-implement selection
//! ourselves (highlighting cells with `Modifier::REVERSED` in the
//! buffer) and ship the selected text back to the system clipboard via
//! the standard OSC 52 protocol so users can paste anywhere with their
//! terminal's normal paste chord.
//!
//! Format: `\x1b]52;c;<base64>\x1b\\` where `c` targets the standard
//! clipboard, `<base64>` is the selected text, and `\x1b\\` (ST) is the
//! string-terminator. Supported by kitty, WezTerm, iTerm2, foot,
//! ghostty, recent gnome-terminal, recent xterm, alacritty (with
//! `enable_clipboard: true` in config since 0.13). Unsupported
//! terminals silently ignore the escape - selection is still visible
//! in-app, just not pushed to the clipboard.
//!
//! tmux passthrough: tmux 3.4+ has `set -g allow-passthrough on` by
//! default; older tmux requires the user to enable it.

use std::io::{Result, Write};

/// Write the OSC 52 sequence to `w` to copy `text` to the system
/// clipboard. `w` should be the same writer the TUI backend uses
/// (typically a stdout handle). Caller is responsible for flushing.
pub fn write_osc52(w: &mut impl Write, text: &str) -> Result<()> {
    // We don't bother chunking long selections - OSC 52 has terminal-
    // dependent length limits (xterm defaults to 8KB, kitty has none
    // configured, gnome-terminal ~1MB). Common code-review-style
    // selections are well under any of these.
    let mut sequence = String::with_capacity(8 + 4 * text.len().div_ceil(3) + 3);
    sequence.push_str("\x1b]52;c;");
    base64_encode_into(&mut sequence, text.as_bytes());
    sequence.push_str("\x1b\\");
    w.write_all(sequence.as_bytes())
}

/// Append base64 encoding of `bytes` to `out`. Standard alphabet,
/// `=`-padded. Hand-rolled (~25 lines) to dodge the dependency-tax of
/// pulling in a base64 crate for one call site.
fn base64_encode_into(out: &mut String, bytes: &[u8]) {
    const A: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b = (u32::from(bytes[i]) << 16)
            | (u32::from(bytes[i + 1]) << 8)
            | u32::from(bytes[i + 2]);
        out.push(A[((b >> 18) & 0x3F) as usize] as char);
        out.push(A[((b >> 12) & 0x3F) as usize] as char);
        out.push(A[((b >> 6) & 0x3F) as usize] as char);
        out.push(A[(b & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b = u32::from(bytes[i]) << 16;
        out.push(A[((b >> 18) & 0x3F) as usize] as char);
        out.push(A[((b >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b = (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8);
        out.push(A[((b >> 18) & 0x3F) as usize] as char);
        out.push(A[((b >> 12) & 0x3F) as usize] as char);
        out.push(A[((b >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
}

#[cfg(test)]
#[path = "../tests/tui/clipboard.rs"]
mod tests;
