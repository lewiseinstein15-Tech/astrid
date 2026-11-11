//! Filesystem tools: read, write, edit, list, grep — all under path
//! confinement, all refusing bad args with honest reasons.

use crate::confinement::resolve_within;
use crate::registry::{Tool, ToolCtx, ToolOutput};
use astrid_core::{Error, Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::{opt_bool, opt_str, opt_usize, req_str};

const READ_LIMIT_BYTES: usize = 100 * 1024;
const WRITE_LIMIT_BYTES: usize = 200 * 1024;
const LS_MAX_ENTRIES: usize = 200;
const LS_MAX_DEPTH: usize = 6;
const GREP_MAX_RESULTS: usize = 50;
const GREP_MAX_FILE_BYTES: u64 = 1024 * 1024;
const GREP_MAX_FILES: usize = 5000;

pub struct FsRead;

#[async_trait]
impl Tool for FsRead {
    fn name(&self) -> &str {
        "fs_read"
    }
    fn description(&self) -> &str {
        "Read a text file inside the workspace (≤100KB; larger files are truncated with a note)."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to the workspace root"}
            },
            "required": ["path"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let path = req_str(self.name(), args, "path")?;
        let resolved = resolve_within(&ctx.confinement_root, &path)?;
        let bytes = fs::read(&resolved)?;
        let total = bytes.len();
        let (text, note) = if total > READ_LIMIT_BYTES {
            (
                String::from_utf8_lossy(&bytes[..READ_LIMIT_BYTES]).into_owned(),
                format!("\n\n[truncated at {READ_LIMIT_BYTES} bytes; file is {total} bytes]"),
            )
        } else {
            (String::from_utf8_lossy(&bytes).into_owned(), String::new())
        };
        Ok(ToolOutput {
            content: format!("{text}{note}"),
            meta_json: Some(json!({"path": resolved.display().to_string(), "bytes": total})),
        })
    }
}

pub struct FsWrite;

#[async_trait]
impl Tool for FsWrite {
    fn name(&self) -> &str {
        "fs_write"
    }
    fn description(&self) -> &str {
        "Write (create or overwrite) a text file inside the workspace; parents are created, content ≤200KB."
    }
    fn risk(&self) -> Risk {
        Risk::Medium
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path relative to the workspace root"},
                "content": {"type": "string", "description": "Full file content to write"}
            },
            "required": ["path", "content"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let path = req_str(self.name(), args, "path")?;
        let content = req_str(self.name(), args, "content")?;
        if content.len() > WRITE_LIMIT_BYTES {
            return Err(Error::ToolRefused {
                tool: self.name().to_string(),
                reason: format!(
                    "content exceeds the {WRITE_LIMIT_BYTES}-byte limit (got {} bytes)",
                    content.len()
                ),
            });
        }
        let resolved = resolve_within(&ctx.confinement_root, &path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&resolved, content.as_bytes())?;
        Ok(ToolOutput {
            content: format!("wrote {} bytes to {}", content.len(), resolved.display()),
            meta_json: Some(
                json!({"path": resolved.display().to_string(), "bytes": content.len()}),
            ),
        })
    }
}

pub struct FsEdit;

#[async_trait]
impl Tool for FsEdit {
    fn name(&self) -> &str {
        "fs_edit"
    }
    fn description(&self) -> &str {
        "Exact-string replace in a workspace file: {path, old, new, replace_all?}. `old` must exist in the file."
    }
    fn risk(&self) -> Risk {
        Risk::Medium
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old": {"type": "string", "description": "Exact text to find"},
                "new": {"type": "string", "description": "Replacement text"},
                "replace_all": {"type": "boolean", "description": "Replace every occurrence (default: first only)"}
            },
            "required": ["path", "old", "new"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let path = req_str(self.name(), args, "path")?;
        let old = req_str(self.name(), args, "old")?;
        let new = req_str(self.name(), args, "new")?;
        let replace_all = opt_bool(self.name(), args, "replace_all")?.unwrap_or(false);
        let resolved = resolve_within(&ctx.confinement_root, &path)?;
        let original = fs::read_to_string(&resolved)?;
        if !original.contains(&old) {
            return Err(Error::ToolRefused {
                tool: self.name().to_string(),
                reason: format!("old string not found in {path}"),
            });
        }
        let occurrences = original.matches(&old).count();
        let updated = if replace_all {
            original.replace(&old, &new)
        } else {
            original.replacen(&old, &new, 1)
        };
        let replaced = if replace_all { occurrences } else { 1 };
        fs::write(&resolved, updated.as_bytes())?;
        Ok(ToolOutput {
            content: format!("replaced {replaced} occurrence(s) of {old:?} in {path}"),
            meta_json: Some(json!({
                "path": resolved.display().to_string(),
                "replacements": replaced,
                "total_occurrences": occurrences,
            })),
        })
    }
}

pub struct FsLs;

#[async_trait]
impl Tool for FsLs {
    fn name(&self) -> &str {
        "fs_ls"
    }
    fn description(&self) -> &str {
        "List a workspace directory (name, size, is_dir); default depth 1, max 200 entries."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory path (default: workspace root)"},
                "depth": {"type": "integer", "description": "Recursion depth (default 1, max 6)"}
            },
            "required": []
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let path = opt_str(self.name(), args, "path")?.unwrap_or_else(|| ".".to_string());
        let depth = opt_usize(self.name(), args, "depth")?
            .unwrap_or(1)
            .max(1)
            .min(LS_MAX_DEPTH);
        let resolved = resolve_within(&ctx.confinement_root, &path)?;

        let mut lines: Vec<String> = Vec::new();
        let truncated = if resolved.is_dir() {
            walk_listing(&resolved, "", depth, &mut lines, LS_MAX_ENTRIES)
        } else {
            let size = fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0);
            lines.push(format!("{} [{} bytes]", resolved.display(), size));
            false
        };
        if truncated {
            lines.push(format!("[listing truncated at {LS_MAX_ENTRIES} entries]"));
        }
        Ok(ToolOutput {
            content: if lines.is_empty() {
                format!("{} is empty", resolved.display())
            } else {
                lines.join("\n")
            },
            meta_json: Some(json!({
                "path": resolved.display().to_string(),
                "entries": lines.len(),
                "depth": depth,
                "truncated": truncated,
            })),
        })
    }
}

/// Depth-first listing; returns `false` when the entry cap stopped the walk.
fn walk_listing(dir: &Path, rel: &str, depth: usize, out: &mut Vec<String>, cap: usize) -> bool {
    if depth == 0 {
        return true;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        out.push(format!("{rel} [unreadable]"));
        return true;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            out.push(format!("{child_rel}/"));
            if out.len() >= cap {
                return false;
            }
            if !walk_listing(&entry.path(), &child_rel, depth - 1, out, cap) {
                return false;
            }
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(format!("{child_rel} [{size} bytes]"));
            if out.len() >= cap {
                return false;
            }
        }
    }
    true
}

pub struct FsGrep;

#[async_trait]
impl Tool for FsGrep {
    fn name(&self) -> &str {
        "fs_grep"
    }
    fn description(&self) -> &str {
        "Regex search over workspace text files (recursive); returns `path:line: text`, max 50 results."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regular expression"},
                "path": {"type": "string", "description": "Directory or file to search (default: workspace root)"},
                "max_results": {"type": "integer", "description": "Cap on matches (default 50, max 50)"}
            },
            "required": ["pattern"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let pattern = req_str(self.name(), args, "pattern")?;
        let path = opt_str(self.name(), args, "path")?.unwrap_or_else(|| ".".to_string());
        let max_results = opt_usize(self.name(), args, "max_results")?
            .unwrap_or(GREP_MAX_RESULTS)
            .max(1)
            .min(GREP_MAX_RESULTS);
        let re = regex::Regex::new(&pattern).map_err(|e| Error::ToolRefused {
            tool: self.name().to_string(),
            reason: format!("invalid regex: {e}"),
        })?;
        let resolved = resolve_within(&ctx.confinement_root, &path)?;

        let mut matches: Vec<String> = Vec::new();
        let mut files_scanned = 0usize;
        if resolved.is_file() {
            grep_file(&resolved, &resolved, &re, max_results, &mut matches);
        } else {
            grep_dir(
                &resolved,
                &resolved,
                &re,
                max_results,
                &mut matches,
                &mut files_scanned,
            );
        }
        Ok(ToolOutput {
            content: if matches.is_empty() {
                format!("no matches for {pattern:?}")
            } else {
                matches.join("\n")
            },
            meta_json: Some(json!({
                "pattern": pattern,
                "matches": matches.len(),
                "files_scanned": files_scanned,
            })),
        })
    }
}

fn grep_dir(
    base: &Path,
    dir: &Path,
    re: &regex::Regex,
    max_results: usize,
    out: &mut Vec<String>,
    scanned: &mut usize,
) {
    if out.len() >= max_results || *scanned >= GREP_MAX_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= max_results || *scanned >= GREP_MAX_FILES {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            // Never follow directory symlinks — avoids cycles.
            if !file_type.is_symlink() {
                grep_dir(base, &entry.path(), re, max_results, out, scanned);
            }
        } else if !file_type.is_symlink() {
            *scanned += 1;
            grep_file(base, &entry.path(), re, max_results, out);
        }
    }
}

fn grep_file(
    base: &Path,
    path: &Path,
    re: &regex::Regex,
    max_results: usize,
    out: &mut Vec<String>,
) {
    let Ok(bytes) = fs::read(path) else { return };
    if bytes.len() as u64 > GREP_MAX_FILE_BYTES {
        return;
    }
    // Cheap binary sniff: NUL byte means skip.
    if bytes.contains(&0u8) {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    let rel = path
        .strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string();
    for (line_no, line) in text.lines().enumerate() {
        if re.is_match(line) {
            out.push(format!(
                "{rel}:{}: {}",
                line_no + 1,
                crate::registry::truncate_chars(line.trim_end(), 200)
            ));
            if out.len() >= max_results {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    async fn ctx_with_root() -> (
        ToolCtx,
        tokio::sync::mpsc::Receiver<astrid_core::Event>,
        std::path::PathBuf,
    ) {
        test_support::make_ctx().await
    }

    #[tokio::test]
    async fn write_read_roundtrip() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        let out = FsWrite
            .execute(
                &json!({"path": "notes/a.txt", "content": "hello astrid"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("wrote 12 bytes"));

        let back = FsRead
            .execute(&json!({"path": "notes/a.txt"}), &ctx)
            .await
            .unwrap();
        assert_eq!(back.content, "hello astrid");
    }

    #[tokio::test]
    async fn edit_replaces_and_refuses_missing_old() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        FsWrite
            .execute(&json!({"path": "doc.md", "content": "foo bar baz"}), &ctx)
            .await
            .unwrap();

        let out = FsEdit
            .execute(&json!({"path": "doc.md", "old": "bar", "new": "qux"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("replaced 1 occurrence"));
        let back = FsRead
            .execute(&json!({"path": "doc.md"}), &ctx)
            .await
            .unwrap();
        assert_eq!(back.content, "foo qux baz");

        let err = FsEdit
            .execute(&json!({"path": "doc.md", "old": "nope", "new": "x"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason.contains("not found"))
        );
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        FsWrite
            .execute(&json!({"path": "multi.txt", "content": "a-b-a-b"}), &ctx)
            .await
            .unwrap();
        let out = FsEdit
            .execute(
                &json!({"path": "multi.txt", "old": "a", "new": "z", "replace_all": true}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("replaced 2 occurrence"));
        let back = FsRead
            .execute(&json!({"path": "multi.txt"}), &ctx)
            .await
            .unwrap();
        assert_eq!(back.content, "z-b-z-b");
    }

    #[tokio::test]
    async fn ls_lists_with_depth() {
        let (ctx, _rx, root) = ctx_with_root().await;
        std::fs::create_dir_all(root.join("deep/inner")).unwrap();
        std::fs::write(root.join("deep/inner/leaf.txt"), "x").unwrap();
        std::fs::write(root.join("top.txt"), "top").unwrap();

        let shallow = FsLs.execute(&json!({"path": "deep"}), &ctx).await.unwrap();
        assert!(shallow.content.contains("inner/"));
        assert!(
            !shallow.content.contains("leaf.txt"),
            "depth 1 must not recurse"
        );

        let deep = FsLs
            .execute(&json!({"path": "deep", "depth": 2}), &ctx)
            .await
            .unwrap();
        assert!(deep.content.contains("inner/leaf.txt"));
    }

    #[tokio::test]
    async fn grep_finds_path_line_text() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        std::fs::create_dir_all(_root.join("src")).unwrap();
        std::fs::write(
            _root.join("src/main.rs"),
            "fn main() {\n    let provider = \"ladder\";\n}\n",
        )
        .unwrap();
        let out = FsGrep
            .execute(&json!({"pattern": "ladder"}), &ctx)
            .await
            .unwrap();
        assert!(
            out.content.contains("src/main.rs:2:"),
            "got: {}",
            out.content
        );
        assert!(out.content.contains("let provider = \"ladder\";"));

        let none = FsGrep
            .execute(&json!({"pattern": "zzz-not-there"}), &ctx)
            .await
            .unwrap();
        assert!(none.content.contains("no matches"));

        let bad = FsGrep
            .execute(&json!({"pattern": "(["}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(bad, Error::ToolRefused { ref reason, .. } if reason.contains("invalid regex"))
        );
    }

    #[tokio::test]
    async fn missing_args_are_refused_with_reason() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        let err = FsRead.execute(&json!({}), &ctx).await.unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: path")
        );
        let err = FsWrite
            .execute(&json!({"path": "x"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: content")
        );
        let err = FsEdit
            .execute(&json!({"path": "x", "old": "y"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: new")
        );
        // Wrong type also refuses.
        let err = FsRead
            .execute(&json!({"path": 42}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("wrong type"));
    }

    #[tokio::test]
    async fn read_truncates_large_files_with_note() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        let big = "x".repeat(READ_LIMIT_BYTES + 500);
        FsWrite
            .execute(&json!({"path": "big.txt", "content": big}), &ctx)
            .await
            .unwrap();
        let out = FsRead
            .execute(&json!({"path": "big.txt"}), &ctx)
            .await
            .unwrap();
        assert!(
            out.content.contains("[truncated at"),
            "got tail: {}",
            &out.content[out.content.len().saturating_sub(120)..]
        );
        assert!(out
            .content
            .contains(format!("file is {} bytes", READ_LIMIT_BYTES + 500).as_str()));
    }

    #[tokio::test]
    async fn write_refuses_oversized_content() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        let big = "y".repeat(WRITE_LIMIT_BYTES + 1);
        let err = FsWrite
            .execute(&json!({"path": "big.txt", "content": big}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason.contains("byte limit")),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn fs_tools_stay_confined() {
        let (ctx, _rx, _root) = ctx_with_root().await;
        let err = FsRead
            .execute(&json!({"path": "../outside.txt"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Confinement(_)));
        let err = FsWrite
            .execute(&json!({"path": "/etc/passwd", "content": "x"}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Confinement(_)));
    }
}
