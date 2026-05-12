//! `edit` tool - exact-string replacement in a file.
//!
//! Mirrors Claude Code's `Edit`: the model supplies `old_string` and
//! `new_string`, and we replace exactly one occurrence. Non-unique matches
//! fail loudly so the model is forced to extend `old_string` with more
//! surrounding context. `replace_all=true` is the explicit opt-out.
//!
//! Writes go straight through the sandbox. A future diff-preview /
//! accept-reject flow can layer on top by reading
//! [`super::ToolContext::auto_apply`].

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::approval::{REJECTION_PREFIX, Verdict};
use crate::diff::unified_diff;
use crate::error::{Error, Result};
use crate::fs::sandboxed;

use super::{Tool, ToolContext};

/// `edit` tool implementation. Stateless - see [`super::Tool`].
#[derive(Debug, Default, Clone, Copy)]
pub struct EditTool;

#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    /// When true, replace every occurrence of `old_string`. When false
    /// or absent, require exactly one occurrence.
    #[serde(default)]
    replace_all: bool,
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replace exactly one occurrence of `old_string` with `new_string` \
         in the file at `path`. The match must be unique unless \
         `replace_all` is true. Path must resolve inside the working \
         directory. Use `read` first if you need to find the exact text."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path":        { "type": "string", "description": "File path (absolute or relative to cwd)." },
                "old_string":  { "type": "string", "description": "Exact text to find. Must be unique unless replace_all." },
                "new_string":  { "type": "string", "description": "Replacement text." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)." }
            },
            "required": ["path", "old_string", "new_string"],
            "additionalProperties": false
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;
        let path = sandboxed(&ctx.cwd, args.path.as_ref())?;

        if args.old_string == args.new_string {
            return Err(Error::Tool(
                "old_string and new_string are identical - nothing to do".into(),
            ));
        }
        if args.old_string.is_empty() {
            return Err(Error::Tool("old_string must not be empty".into()));
        }

        let original = tokio::fs::read_to_string(&path).await?;
        let count = original.matches(&args.old_string).count();

        let new_contents = if args.replace_all {
            if count == 0 {
                return Err(Error::Tool("old_string not found".into()));
            }
            original.replace(&args.old_string, &args.new_string)
        } else {
            match count {
                0 => return Err(Error::Tool("old_string not found".into())),
                1 => original.replacen(&args.old_string, &args.new_string, 1),
                n => {
                    return Err(Error::Tool(format!(
                        "old_string is not unique ({n} occurrences). Add \
                         surrounding context, or set replace_all=true."
                    )));
                }
            }
        };

        // Defensive: even with `old_string != new_string`, edge cases
        // (no occurrences slipping through, identical-after-trim
        // anomalies, ...) can leave new_contents byte-equal to the
        // original. Skip the gate + skip the write rather than
        // burning an approval prompt on nothing.
        if original == new_contents {
            return Ok(format!("no change applied to {}", path.display()));
        }

        let diff = unified_diff(&original, &new_contents, &path);
        if ctx.gate.review_diff(&path, &diff).await == Verdict::Reject {
            return Ok(format!(
                "{REJECTION_PREFIX} edit to {} was NOT applied. File unchanged.",
                path.display()
            ));
        }

        tokio::fs::write(&path, &new_contents).await?;

        let n = if args.replace_all { count } else { 1 };
        // Plural "s" only when the count actually justifies it. Positive
        // form (`if x && y`) keeps clippy::if_not_else (pedantic) quiet.
        let plural = if n > 1 { "s" } else { "" };
        Ok(format!("edited {} ({n} replacement{plural})", path.display()))
    }
}

#[cfg(test)]
#[path = "../tests/tools/edit.rs"]
mod tests;
