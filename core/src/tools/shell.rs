//! `shell` tool - run a command in the working directory.
//!
//! Cross-platform: dispatches to `sh -c` on Unix and `cmd /C` on Windows.
//! The tool name is `shell` (not `bash`) because the actual interpreter
//! varies by platform; the agent shouldn't assume bashisms.
//!
//! Output is the combined stdout + stderr alongside the exit status,
//! formatted as plain text for the model to consume. A timeout is
//! enforced via [`tokio::time::timeout`]; on expiry we kill the child
//! and report the partial output.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::approval::{REJECTION_PREFIX, Verdict};
use crate::error::{Error, Result};

use super::{Tool, ToolContext};

/// Default timeout when the model omits `timeout_seconds`.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Hard cap so a buggy agent can't request a 24-hour shell. 10 minutes
/// is enough for most build/test runs; longer-running ops should be
/// orchestrated externally.
const MAX_TIMEOUT_SECS: u64 = 600;

/// Truncate captured stdout/stderr to keep one runaway command from
/// blowing the model's context window. Roughly 200 KiB total per stream.
const MAX_STREAM_BYTES: usize = 200 * 1024;

/// `shell` tool implementation. Stateless - see [`super::Tool`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ShellTool;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command in the working directory. Uses `sh -c` on \
         Unix and `cmd /C` on Windows. Returns combined stdout + stderr and \
         the exit status. Default timeout is 30s; max 600s. Don't assume \
         bash-only syntax."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command":         { "type": "string", "description": "Command to run, as a single shell string." },
                "timeout_seconds": { "type": "integer", "minimum": 1, "description": "Kill the command after this many seconds (default 30, max 600)." }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;

        if args.command.trim().is_empty() {
            return Err(Error::Tool("command is empty".into()));
        }

        // Every shell command is reviewed regardless of `auto_apply`
        // mode. `Never` and `Safe` both route through the gate; only
        // the future per-command allowlist (`/allow <pattern>`, post-
        // v0.1) will skip review for explicitly trusted shells. The
        // model receives a "user rejected" tool result on rejection
        // so it can plan around the refusal without retrying the
        // same command.
        //
        // `_ = ctx.auto_apply()` is intentionally dropped: the field
        // exists on `ToolContext` for the diff path (Write/Edit) and
        // is read by the approval gate for those flows; for shell we
        // always prompt until allowlisting lands.
        if ctx.gate.review_shell(&args.command).await == Verdict::Reject {
            return Ok(format!(
                "{REJECTION_PREFIX} shell command was NOT executed. Command: {}",
                args.command
            ));
        }

        let secs = args
            .timeout_seconds
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);

        // `cfg!(windows)` (runtime, not `#[cfg(windows)]`) lets the same
        // function body cover both platforms with one branch - fine here
        // because the platform doesn't change at runtime and the dead
        // branch optimizes away.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&args.command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&args.command);
            c
        };

        cmd.current_dir(&ctx.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // `kill_on_drop` ties the child's lifetime to the
            // `tokio::process::Child` handle - if we drop it on timeout
            // (or the future is cancelled), the process gets SIGKILL on
            // Unix / `TerminateProcess` on Windows.
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Read stdout/stderr concurrently with the wait. Two reasons:
        // 1. A producer that fills the kernel pipe buffer (~64 KiB)
        //    blocks until someone drains it. The old wait-then-read
        //    shape deadlocked any process that printed more than
        //    that before exiting.
        // 2. `take(cap)` bounds each stream at the source, so a
        //    runaway producer can't OOM us before format_output runs
        //    its post-hoc truncation. The +1 lets `truncate_lossy`
        //    detect that more was available than we kept.
        let cap = (MAX_STREAM_BYTES as u64).saturating_add(1);
        let read_out = async {
            let mut buf = Vec::new();
            if let Some(s) = stdout {
                s.take(cap).read_to_end(&mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let read_err = async {
            let mut buf = Vec::new();
            if let Some(s) = stderr {
                s.take(cap).read_to_end(&mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let waited = async { child.wait().await };

        // Run the join under a timeout. On expiry the future is dropped,
        // `kill_on_drop` fires, and we report a timeout result.
        let run = async {
            let (out, err, status) = tokio::try_join!(read_out, read_err, waited)?;
            Ok::<_, std::io::Error>((status, out, err))
        };

        match timeout(Duration::from_secs(secs), run).await {
            Ok(Ok((status, out, err))) => Ok(format_output(
                status.code(),
                status.success(),
                false,
                &out,
                &err,
            )),
            Ok(Err(io_err)) => Err(io_err.into()),
            Err(_elapsed) => Ok(format_output(None, false, true, &[], &[])),
        }
    }
}

fn format_output(
    code: Option<i32>,
    success: bool,
    timed_out: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let stdout_s = truncate_lossy(stdout);
    let stderr_s = truncate_lossy(stderr);

    let status_line = if timed_out {
        "exit: timed out".to_string()
    } else if let Some(c) = code {
        format!("exit: {c}{}", if success { "" } else { " (failed)" })
    } else {
        "exit: signaled".to_string()
    };

    let mut out = String::new();
    out.push_str(&status_line);
    out.push('\n');
    if !stdout_s.is_empty() {
        out.push_str("--- stdout ---\n");
        out.push_str(&stdout_s);
        if !stdout_s.ends_with('\n') {
            out.push('\n');
        }
    }
    if !stderr_s.is_empty() {
        out.push_str("--- stderr ---\n");
        out.push_str(&stderr_s);
        if !stderr_s.ends_with('\n') {
            out.push('\n');
        }
    }
    if stdout_s.is_empty() && stderr_s.is_empty() && !timed_out {
        out.push_str("(no output)\n");
    }
    out
}

fn truncate_lossy(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_STREAM_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let head = String::from_utf8_lossy(&bytes[..MAX_STREAM_BYTES]).into_owned();
    format!(
        "{head}\n... (truncated, {} more bytes)",
        bytes.len() - MAX_STREAM_BYTES
    )
}

#[cfg(test)]
#[path = "../tests/tools/shell.rs"]
mod tests;
