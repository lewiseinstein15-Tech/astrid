//! The sync ledger: a JSON map of `relative path → sha256 hex`.
//!
//! `manifest.json` lives at the memory root and mirrors every file under the
//! page directories. It is synced on [`crate::MemoryKernel::open`] (new and
//! changed files are hashed in) and re-checked by
//! [`crate::MemoryKernel::verify`], which reports drift and then updates the
//! manifest to the current disk state.

use crate::store::{self, PAGE_DIRS};
use astrid_core::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::Path;

/// Name of the manifest file at the memory root.
pub const MANIFEST_NAME: &str = "manifest.json";

/// `relative path → sha256 hex` ledger, deterministically ordered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(flatten)]
    pub map: BTreeMap<String, String>,
}

/// Lowercase hex encoding of a sha256 digest.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

impl Manifest {
    /// Load `root/manifest.json`. A missing file yields an empty manifest;
    /// a corrupt one is an error (the caller decides to rebuild).
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(MANIFEST_NAME);
        match std::fs::read_to_string(&path) {
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write the manifest back to `root/manifest.json`.
    pub fn save(&self, root: &Path) -> Result<()> {
        let raw = serde_json::to_string_pretty(&self.map)?;
        std::fs::write(root.join(MANIFEST_NAME), raw + "\n")?;
        Ok(())
    }

    /// sha256 hex of one file, streamed.
    pub fn hash_file(path: &Path) -> Result<String> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex(&hasher.finalize()))
    }

    /// Hash every regular file under the page directories
    /// (`notes/`, `entities/`, `summaries/`), keyed by root-relative path.
    pub fn scan_disk(root: &Path) -> Result<BTreeMap<String, String>> {
        let mut map = BTreeMap::new();
        for dir in PAGE_DIRS {
            let dir_path = root.join(dir);
            let entries = match std::fs::read_dir(&dir_path) {
                Ok(entries) => entries,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let path = entry?.path();
                if path.is_file() {
                    let rel = store::rel_path(root, &path);
                    map.insert(rel, Self::hash_file(&path)?);
                }
            }
        }
        Ok(map)
    }

    /// Drift between this manifest and the current disk state.
    ///
    /// Messages: `missing:` (manifest entry with no file), `changed:` (hash
    /// mismatch), `untracked:` (file with no manifest entry).
    pub fn diff(&self, disk: &BTreeMap<String, String>) -> Vec<String> {
        let mut drift = Vec::new();
        for (path, expected) in &self.map {
            match disk.get(path) {
                None => drift.push(format!("missing: {path}")),
                Some(actual) if actual != expected => {
                    drift.push(format!("changed: {path}"));
                }
                Some(_) => {}
            }
        }
        for path in disk.keys() {
            if !self.map.contains_key(path) {
                drift.push(format!("untracked: {path}"));
            }
        }
        drift.sort();
        drift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astrid-manifest-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hash_is_deterministic_sha256_hex() {
        let root = temp_root("hash");
        let file = root.join("a.md");
        std::fs::write(&file, "content").unwrap();
        let h1 = Manifest::hash_file(&file).unwrap();
        let h2 = Manifest::hash_file(&file).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        // sha256("content"), verified externally
        assert_eq!(
            h1,
            "ed7002b439e9ac845f22357d822bac1444730fbdb6016d3ec9432297b9ec9f73"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_reports_missing_changed_untracked() {
        let mut manifest = Manifest::default();
        let mut disk = BTreeMap::new();
        disk.insert("notes/a.md".to_string(), "aaa".to_string());
        disk.insert("notes/b.md".to_string(), "bbb".to_string());
        disk.insert("notes/new.md".to_string(), "fresh".to_string());
        manifest
            .map
            .insert("notes/a.md".to_string(), "aaa".to_string());
        manifest
            .map
            .insert("notes/gone.md".to_string(), "zzz".to_string());
        manifest
            .map
            .insert("notes/b.md".to_string(), "different".to_string());

        let drift = manifest.diff(&disk);
        assert!(drift.contains(&"missing: notes/gone.md".to_string()));
        assert!(drift.contains(&"changed: notes/b.md".to_string()));
        assert!(drift.contains(&"untracked: notes/new.md".to_string()));
        assert_eq!(drift.len(), 3);
    }

    #[test]
    fn save_load_roundtrip() {
        let root = temp_root("roundtrip");
        let mut manifest = Manifest::default();
        manifest
            .map
            .insert("notes/x-11111111.md".into(), "deadbeef".into());
        manifest.save(&root).unwrap();
        let loaded = Manifest::load(&root).unwrap();
        assert_eq!(loaded, manifest);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_manifest_loads_empty() {
        let root = temp_root("empty");
        let manifest = Manifest::load(&root).unwrap();
        assert!(manifest.map.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
