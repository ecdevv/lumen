//! `write` tool - overwrite (or create) a file with the given content.
//!
//! Writes happen unconditionally inside the sandbox. The
//! [`super::ToolContext`] carries an `auto_apply` policy that future
//! diff-preview / accept-reject flows will gate on.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::approval::{REJECTION_PREFIX, Verdict};
use crate::diff::unified_diff;
use crate::error::Result;
use crate::fs::sandboxed;

use super::{Tool, ToolContext};

/// `write` tool implementation. Stateless - see [`super::Tool`].
#[derive(Debug, Default, Clone, Copy)]
pub struct WriteTool;

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write text content to a file, overwriting any existing content. \
         Creates parent directories as needed. Path must resolve inside \
         the working directory."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path":    { "type": "string", "description": "File path (absolute or relative to cwd)." },
                "content": { "type": "string", "description": "New full file contents." }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    async fn invoke(&self, ctx: &ToolContext, args_json: &str) -> Result<String> {
        let args: Args = serde_json::from_str(args_json)?;
        let path = sandboxed(&ctx.cwd, args.path.as_ref())?;

        // Read the current content for the diff. NotFound means we're
        // creating a new file - treat that as "empty old" so the diff
        // renders every line as an addition.
        let original = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };

        // Byte-identical proposed content = no-op. Skip the gate
        // (nothing to review) and skip the write (nothing to do).
        if original == args.content {
            return Ok(format!(
                "no change: {} already has the requested content",
                path.display()
            ));
        }

        let diff = unified_diff(&original, &args.content, &path);
        if ctx.gate.review_diff(&path, &diff).await == Verdict::Reject {
            // Declarative + unambiguous: small local models tend to
            // gloss over softer phrasings ("user rejected ...") and
            // claim success anyway. The capitalized REJECTED token
            // pairs with the system prompt's directive that tool
            // results are authoritative.
            return Ok(format!(
                "{REJECTION_PREFIX} write to {} was NOT performed. File unchanged.",
                path.display()
            ));
        }

        // `create_dir_all` is a no-op when the directory already exists,
        // so it's safe to call unconditionally on every write.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &args.content).await?;

        Ok(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            path.display()
        ))
    }
}

#[cfg(test)]
#[path = "../tests/tools/write.rs"]
mod tests;
