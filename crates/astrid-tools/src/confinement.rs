//! Path confinement (docs/PROTOCOLS.md §4): every filesystem tool resolves
//! its paths through [`resolve_within`] — canonicalized, segment-boundary
//! checked, no `..` escape, symlink-resolved. Missing targets are allowed by
//! resolving the deepest existing ancestor and appending the rest, so
//! `fs_write` to a brand-new file works while still confinement-checked.

use astrid_core::{Error, Result};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Resolve `path` (absolute or relative) strictly inside `root`.
///
/// Rules:
/// - relative paths join onto the canonical root;
/// - `..` segments that would walk above the root are refused outright;
/// - absolute paths are allowed only when they land inside the root;
/// - symlinks are followed, but the resolved target must stay inside the root;
/// - a not-yet-existing target resolves against its deepest existing ancestor
///   so new files and directories can be created safely.
pub fn resolve_within(root: &Path, path: &str) -> Result<PathBuf> {
    let canonical_root = root.canonicalize().map_err(|e| {
        Error::Confinement(format!(
            "confinement root {} cannot be resolved: {e}",
            root.display()
        ))
    })?;

    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok(canonical_root);
    }

    let raw = Path::new(trimmed);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        canonical_root.join(raw)
    };
    let normalized = lexical_normalize(&joined)?;
    if !normalized.starts_with(&canonical_root) {
        return Err(Error::Confinement(format!(
            "path `{path}` escapes the confinement root {}",
            canonical_root.display()
        )));
    }

    let resolved = resolve_longest(&normalized)?;
    if !resolved.starts_with(&canonical_root) {
        return Err(Error::Confinement(format!(
            "path `{path}` resolves (symlink) outside the confinement root {}",
            canonical_root.display()
        )));
    }
    Ok(resolved)
}

/// Resolve `.` and `..` lexically. A `..` that pops past the filesystem root
/// is refused rather than silently clamped.
fn lexical_normalize(p: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(Error::Confinement(
                        "path walks above its root via `..`".to_string(),
                    ));
                }
            }
            component => out.push(component.as_os_str()),
        }
    }
    Ok(out)
}

/// Canonicalize the deepest existing ancestor and append the missing tail.
fn resolve_longest(p: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = p.canonicalize() {
        return Ok(canonical);
    }
    let mut tail: Vec<OsString> = Vec::new();
    let mut current = p.to_path_buf();
    while !current.exists() {
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) if parent != current => {
                tail.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => {
                return current
                    .canonicalize()
                    .map_err(|e| Error::Confinement(format!("cannot resolve path: {e}")));
            }
        }
    }
    let base = current
        .canonicalize()
        .map_err(|e| Error::Confinement(format!("cannot resolve existing ancestor: {e}")))?;
    let mut resolved = base;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astrid-confine-{tag}-{}-{}",
            std::process::id(),
            astrid_core::ids::new_id("t").replace('_', "")
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_and_dot_resolve_to_root() {
        let root = temp_root("dot");
        assert_eq!(
            resolve_within(&root, "").unwrap(),
            root.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_within(&root, ".").unwrap(),
            root.canonicalize().unwrap()
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn relative_path_joins_and_canonicalizes() {
        let root = temp_root("join");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/file.txt"), "hi").unwrap();
        let resolved = resolve_within(&root, "sub/file.txt").unwrap();
        assert_eq!(resolved, root.join("sub/file.txt").canonicalize().unwrap());
        // Dots are fine when they do not escape.
        assert_eq!(
            resolve_within(&root, "./sub/../sub/file.txt").unwrap(),
            resolved
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_target_resolves_via_existing_ancestor() {
        let root = temp_root("missing");
        let resolved = resolve_within(&root, "newdir/deeper/file.txt").unwrap();
        assert_eq!(resolved, root.join("newdir/deeper/file.txt"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parent_escape_refused() {
        let root = temp_root("parent");
        let err = resolve_within(&root, "../escape").unwrap_err();
        assert!(err.to_string().contains("escapes"), "got: {err}");
        // Multi-hop escapes too.
        assert!(resolve_within(&root, "sub/../../outside").is_err());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn absolute_outside_refused() {
        let root = temp_root("abs");
        let err = resolve_within(&root, "/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("escapes"), "got: {err}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn absolute_inside_allowed() {
        let root = temp_root("absin");
        fs::create_dir_all(root.join("sub")).unwrap();
        let inside = root.join("sub").canonicalize().unwrap();
        assert_eq!(
            resolve_within(&root, inside.to_str().unwrap()).unwrap(),
            inside
        );
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_refused() {
        use std::os::unix::fs::symlink;
        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        fs::write(outside.join("secret.txt"), "nope").unwrap();
        symlink(&outside, root.join("door")).unwrap();
        // Direct hop through the symlink.
        let err = resolve_within(&root, "door").unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
        // And through the symlink into a child path.
        assert!(resolve_within(&root, "door/secret.txt").is_err());
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn unresolvable_root_refused() {
        let bogus = std::env::temp_dir().join("astrid-no-such-root-xyz");
        assert!(resolve_within(&bogus, "x").is_err());
    }
}
