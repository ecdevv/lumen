//! `read` tool - read a file's contents, with optional line-range slicing.
//!
//! Output is line-numbered (1-based, fixed-width column) so the model can
//! reference exact lines back in subsequent edit calls. Binary content is
//! tolerated via lossy UTF-8 decoding - replacement chars in, no panic.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Result};
use crate::fs::sandboxed;

use super::{Tool, ToolContext};

/// Default cap when the model omits `limit`. 2000 lines roughly matches
/// what Claude Code uses; large enough to skim, small enough to never
/// blow the context window on a single call.
const DEFAULT_LIMIT: usize = 2000;

/// Hard cap regardless of what the model asks for, to prevent a runaway
/// `limit: 1_000_000_000` from materializing a massive `String`.
const MAX_LIMIT: usize = 50_000;

/// Hard cap on file size we'll materialize into memory. Past this we
/// refuse the read rather than risk OOM on a runaway path. ~10 MiB is
/// enough for any sane source file; binaries or logs that exceed it
/// should be inspected via `grep` or via `shell` with `head` / `sed`.
//
// Enforced at the metadata layer, *before* `tokio::fs::read`. The
// `MAX_LIMIT` line-count cap fires after the read, so it's no help
// against an adversarial path - the OOM happens before we slice.
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// `read` tool implementation. Stateless - see [`super::Tool`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ReadTool;

#[derive(Deserialize)]
struct Args {
    path: String,
    /// 0-based line offset. `None` means start at the top of the file.
    #[serde(default)]
    offset: Option<usize>,
    /// Max number of lines to return. `None` => [`DEFAULT_LIMIT`].
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Paths are resolved relative to the working \
         directory and may not escape it. Output is line-numbered. Use `offset` \
         (0-based) and `limit` to read a slice of large files."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path (absolute or relative to cwd)."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "0-based line index to start from."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum number of lines to return."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;
        let path = sandboxed(&ctx.cwd, args.path.as_ref())?;

        // Stat first so a multi-GB path can't OOM us via the full read.
        let meta = tokio::fs::metadata(&path).await?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(Error::Tool(format!(
                "file is {} bytes (cap {MAX_FILE_BYTES}); use `grep` or `shell` (head/sed) for partial inspection",
                meta.len()
            )));
        }

        let bytes = tokio::fs::read(&path).await?;
        // Lossy decode: any non-UTF-8 bytes become U+FFFD instead of an
        // error. Reading binary files is rare, and a noisy result is
        // more useful to the model than a hard failure.
        let text = String::from_utf8_lossy(&bytes);

        let offset = args.offset.unwrap_or(0);
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

        // Stream through `text.lines()` twice instead of collecting
        // into `Vec<&str>` once. Each `&str` slot is 16 bytes; a 10
        // MiB file of single-byte lines would otherwise allocate
        // ~160 MB of Vec backing storage just to hold the borrowed
        // line pointers. Two O(n) passes (count, then slice) are
        // worth that memory ceiling.
        let total = text.lines().count();

        if offset > total {
            return Err(Error::Tool(format!(
                "offset {offset} is past end of file ({total} lines)"
            )));
        }

        let end = (offset + limit).min(total);
        let take_count = end - offset;
        let mut out = String::with_capacity(text.len().min(take_count * 80));
        // `enumerate` *before* `skip`/`take` so `i` carries the
        // global 0-based line index, not a slice-local one.
        for (i, line) in text
            .lines()
            .enumerate()
            .skip(offset)
            .take(take_count)
        {
            // 1-based display line numbers, right-padded to 6 cols.
            // Matches the shape of `cat -n` and existing CC output.
            use std::fmt::Write;
            let _ = writeln!(out, "{:>6}\t{line}", i + 1);
        }

        if end < total {
            use std::fmt::Write;
            let _ = writeln!(
                out,
                "... ({} more lines; pass `offset: {end}` to continue)",
                total - end
            );
        }

        Ok(out)
    }
}

#[cfg(test)]
#[path = "../tests/tools/read.rs"]
mod tests;
