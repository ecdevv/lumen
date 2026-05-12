//! Filesystem helpers shared across the crate.
//!
//! Path sandboxing: every file-touching tool resolves user-supplied
//! paths through [`sandboxed`] before any `read` / `write`, so the
//! agent can't be coaxed into reading `/etc/passwd` via `../../etc/passwd`
//! or an absolute path outside the working tree.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Resolve `target` against `cwd` and reject if the result escapes `cwd`.
///
/// Works on paths that don't exist yet (Write needs this), so we hand-roll
/// `..` / `.` resolution instead of calling `canonicalize`.
///
/// Symlinks inside `cwd` that point outside are *not* detected here - the
/// OS follows them at access time. Closing that gap is a defense-in-depth
/// task for later hardening.
pub fn sandboxed(cwd: &Path, target: &Path) -> Result<PathBuf> {
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        cwd.join(target)
    };

    let normalized = normalize(&joined);
    let cwd_norm = normalize(cwd);

    // `Path::starts_with` matches whole components, so it correctly
    // distinguishes `/a/b` from `/a/bc` (lexical `&str::starts_with`
    // would not).
    if !normalized.starts_with(&cwd_norm) {
        return Err(Error::Tool(format!(
            "path `{}` escapes the working directory `{}`",
            normalized.display(),
            cwd_norm.display()
        )));
    }
    Ok(normalized)
}

/// Resolve `..` and `.` components without touching the filesystem.
///
/// Pure path arithmetic - kept private because callers should always go
/// through [`sandboxed`], which combines normalization with the escape check.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            // `RootDir`, `Prefix` (Windows drive letters), and `Normal`
            // segments all flow through unchanged - we only collapse the
            // synthetic `.` / `..` components.
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/fs.rs"]
mod tests;
