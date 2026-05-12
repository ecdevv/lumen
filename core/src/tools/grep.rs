//! `grep` tool - wraps the system `ripgrep` (`rg`) binary.
//!
//! Spawns `rg --json` and parses match events as they stream in. ripgrep
//! must be on `PATH` at runtime; the AUR `PKGBUILD` declares it as
//! `Depends`, and on Windows it's installable via winget / scoop /
//! chocolatey. Shelling out keeps lumen's own surface area small and
//! gets us automatic parity with ripgrep's matching, walking, and
//! `.gitignore` semantics.
//!
//! Output: one match per line, formatted `path:line:content`. Paths are
//! displayed relative to the working directory when possible.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::fs::sandboxed;

use super::{Tool, ToolContext};

/// Default cap on returned matches when the model omits `max_results`.
const DEFAULT_MAX_RESULTS: usize = 200;
/// Hard cap regardless of model input.
const MAX_MAX_RESULTS: usize = 5_000;

/// `grep` tool implementation. Stateless - see [`super::Tool`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GrepTool;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    /// Subdirectory or file to search within. Defaults to cwd. Must
    /// resolve inside the sandbox.
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search the working tree for a regex pattern using ripgrep. Honors \
         .gitignore. Returns matches as `path:line:content`, one per line. \
         Use `path` to scope to a subdirectory or single file; \
         `case_insensitive` for /i; `max_results` to bound output. Requires \
         the `rg` binary to be installed on PATH."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern":          { "type": "string", "description": "Rust-flavored regex (ripgrep's default engine)." },
                "path":             { "type": "string", "description": "File or directory to search (defaults to cwd)." },
                "case_insensitive": { "type": "boolean", "description": "Case-insensitive matching." },
                "max_results":      { "type": "integer", "minimum": 1, "description": "Cap on returned matches." }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;

        let root = if let Some(p) = args.path.as_deref() {
            sandboxed(&ctx.cwd, Path::new(p))?
        } else {
            ctx.cwd.clone()
        };

        if !root.exists() {
            return Err(Error::Tool(format!(
                "path does not exist: {}",
                root.display()
            )));
        }

        let max = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(MAX_MAX_RESULTS);

        let mut cmd = Command::new("rg");
        cmd.arg("--json");
        if args.case_insensitive {
            cmd.arg("--ignore-case");
        }
        // `--regexp PATTERN` keeps the pattern from being mis-parsed as
        // a flag if it starts with `-` (e.g. `--foo`).
        cmd.arg("--regexp").arg(&args.pattern).arg(&root);
        cmd.current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Tie the child's lifetime to its handle: if the future is
            // dropped (caller cancels, panic), rg gets killed too.
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            // Actionable error message: distinguish "rg isn't installed"
            // from "rg crashed", because the model can't fix the former.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Tool(
                    "ripgrep (`rg`) is not on PATH. Install it: \
                     https://github.com/BurntSushi/ripgrep#installation"
                        .into(),
                ));
            }
            Err(e) => return Err(e.into()),
        };

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take();

        let mut hits: Vec<String> = Vec::new();
        let mut lines = BufReader::new(stdout).lines();

        while let Some(line) = lines.next_line().await? {
            if hits.len() >= max {
                break;
            }
            // Per-line parse failure is non-fatal: rg might emit
            // unfamiliar event shapes across versions, and a single bad
            // line shouldn't sink the search.
            let Ok(event) = serde_json::from_str::<RgLine>(&line) else {
                continue;
            };
            if event.ty != "match" {
                continue;
            }
            let Some(data) = event.data else { continue };
            let Some(path) = data.path.and_then(|p| p.text) else {
                continue;
            };
            let Some(content) = data.lines.and_then(|l| l.text) else {
                continue;
            };
            let line_no = data.line_number.unwrap_or(0);
            let trimmed = content.trim_end_matches(['\n', '\r']);

            // Display relative to cwd for compactness; absolute path is
            // a fallback for unusual layouts (symlinks crossing roots).
            let rel = Path::new(&path)
                .strip_prefix(&ctx.cwd)
                .map_or_else(|_| path.clone(), |p| p.display().to_string());
            hits.push(format!("{rel}:{line_no}:{trimmed}"));
        }

        let status = child.wait().await?;
        // ripgrep exit codes:
        //   0 = at least one match
        //   1 = no matches (intentional, not an error)
        //   2 = real error (bad regex, IO, etc.)
        if status.code().is_some_and(|c| c >= 2) {
            let mut err_text = String::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.read_to_string(&mut err_text).await;
            }
            return Err(Error::Tool(format!(
                "rg failed (exit {}): {}",
                status.code().unwrap_or(-1),
                err_text.trim()
            )));
        }

        if hits.is_empty() {
            return Ok("(no matches)".into());
        }
        let mut out = hits.join("\n");
        if hits.len() >= max {
            let _ = write!(
                out,
                "\n... (capped at {max} matches; refine pattern or raise max_results)"
            );
        }
        Ok(out)
    }
}

/// One line of `rg --json` output.
//
// Typed deserialization rather than ad-hoc `serde_json::Value` lookups so
// each field's expected shape is stated once. Unknown event types fall
// through to the `ty != "match"` filter - no `#[serde(other)]` needed
// because we use `serde_json::Value`-free typed structs.
#[derive(Deserialize)]
struct RgLine {
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    data: Option<RgData>,
}

#[derive(Deserialize)]
struct RgData {
    #[serde(default)]
    path: Option<RgText>,
    #[serde(default)]
    lines: Option<RgText>,
    #[serde(default)]
    line_number: Option<u64>,
}

/// rg emits `{"text": "..."}` for valid UTF-8 and `{"bytes": "..base64..."}`
/// otherwise. We only handle the UTF-8 case; non-UTF-8 paths /
/// match content are rare on dev machines and gracefully fall through as
/// `None` (the match is skipped).
#[derive(Deserialize)]
struct RgText {
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
#[path = "../tests/tools/grep.rs"]
mod tests;
