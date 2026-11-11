//! astrid-memory — five-verb protocol, markdown-backed store, hybrid search.
//!
//! The public surface is the [`MemoryKernel`] and the frozen protocol types
//! in [`protocol`]. Pages are markdown files under
//! `<root>/{notes,entities,summaries}/` with a `---` fenced frontmatter
//! block; [`manifest`] keeps a sha256 sync ledger over them and [`search`]
//! provides BM25-lite + recency + exact-phrase retrieval fused with RRF.
//!
//! Storage layout (ARCHITECTURE.md §8):
//!
//! ```text
//! <data_dir>/memory/
//! ├── manifest.json    # path → sha256 sync ledger
//! ├── notes/           # remember-verb pages
//! ├── entities/        # entity-verb pages
//! └── summaries/       # synthesize-verb pages
//! ```

pub mod manifest;
pub mod protocol;
pub mod search;
pub mod store;

use astrid_core::{ids, Error, Result};
use protocol::{EntityPage, PageSummary, RecallHit, Tombstone};
use std::path::{Path, PathBuf};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

pub use protocol::PROTOCOL_VERSION;

/// Kinds accepted by [`MemoryKernel::remember`].
pub const REMEMBER_KINDS: &[&str] = &["note", "reference", "log"];

struct KernelState {
    pages: Vec<store::Page>,
    manifest: manifest::Manifest,
}

/// The memory kernel: the only door into Astrid's memory.
///
/// All methods take `&self`; internal state (page cache + manifest) is
/// guarded by an [`std::sync::RwLock`]. Content is never deleted — `forget`
/// tombstones, everything else appends or rewrites through the verbs.
pub struct MemoryKernel {
    root: PathBuf,
    state: RwLock<KernelState>,
}

impl MemoryKernel {
    /// Open (or create) the memory root: `<root>/{notes,entities,summaries}`
    /// and `manifest.json`. Every open re-syncs the manifest with the disk:
    /// new and changed files are hashed in, and drift is logged, never
    /// hidden.
    pub fn open(root: &Path) -> Result<Self> {
        for dir in store::PAGE_DIRS {
            std::fs::create_dir_all(root.join(dir))?;
        }
        let pages = store::load_pages(root)?;
        let manifest = match manifest::Manifest::load(root) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("memory: manifest unreadable, rebuilding from disk: {e}");
                manifest::Manifest::default()
            }
        };
        let disk = manifest::Manifest::scan_disk(root)?;
        for drift in manifest.diff(&disk) {
            tracing::warn!("memory: sync on open: {drift}");
        }
        let mut manifest = manifest;
        let changed = manifest.map != disk;
        manifest.map = disk;
        if changed || !root.join(manifest::MANIFEST_NAME).exists() {
            manifest.save(root)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            state: RwLock::new(KernelState { pages, manifest }),
        })
    }

    fn read_state(&self) -> RwLockReadGuard<'_, KernelState> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, KernelState> {
        self.state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Re-hash one page file into the manifest and persist it.
    fn sync_file(&self, state: &mut KernelState, rel: &str) -> Result<()> {
        let hash = manifest::Manifest::hash_file(&self.root.join(rel))?;
        state.manifest.map.insert(rel.to_string(), hash);
        state.manifest.save(&self.root)
    }

    /// `remember(topic, body, kind) -> PageId` — write a markdown page under
    /// `notes/`. `kind` must be one of `note`, `reference`, `log`.
    pub fn remember(&self, topic: &str, body: &str, kind: &str) -> Result<String> {
        let kind = kind.trim();
        if !REMEMBER_KINDS.contains(&kind) {
            return Err(Error::Memory(format!(
                "invalid kind `{kind}` (expected one of {})",
                REMEMBER_KINDS.join(", ")
            )));
        }
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(Error::Memory("topic must not be empty".into()));
        }
        let id = ids::new_id("page");
        let page = store::Page::new(&id, topic, kind, "notes", body);
        store::save(&self.root, &page)?;
        let mut state = self.write_state();
        state.pages.push(page.clone());
        self.sync_file(&mut state, &page.path)?;
        Ok(id)
    }

    /// `recall(query, k) -> Vec<RecallHit>` — BM25-lite + recency +
    /// exact-phrase, fused with deterministic RRF. Forgotten pages are
    /// never recalled. An empty/tokenless query returns the newest pages.
    pub fn recall(&self, query: &str, k: usize) -> Result<Vec<RecallHit>> {
        let state = self.read_state();
        Ok(search::search(query, &state.pages, k))
    }

    /// `entity(name) -> Option<EntityPage>` — an entity page matched by
    /// exact name or alias (both case-insensitive).
    pub fn entity(&self, name: &str) -> Result<Option<EntityPage>> {
        let state = self.read_state();
        let target = name.trim();
        if target.is_empty() {
            return Ok(None);
        }
        let found = state.pages.iter().find(|p| {
            p.kind == "entity"
                && !p.forgotten
                && (p.title.eq_ignore_ascii_case(target)
                    || p.aliases.iter().any(|a| a.eq_ignore_ascii_case(target)))
        });
        Ok(found.map(|p| EntityPage {
            id: p.id.clone(),
            name: p.title.clone(),
            kind: p.kind.clone(),
            aliases: p.aliases.clone(),
            edges: p.edges.clone(),
        }))
    }

    /// `synthesize(topic, source_ids, body) -> PageId` — write a summary
    /// page under `summaries/` whose frontmatter `sources:` links the
    /// source page ids (deduplicated, in order).
    pub fn synthesize(&self, topic: &str, source_ids: &[String], body: &str) -> Result<String> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err(Error::Memory("topic must not be empty".into()));
        }
        let mut sources = Vec::new();
        for id in source_ids {
            let id = id.trim();
            if !id.is_empty() && !sources.iter().any(|s: &String| s == id) {
                sources.push(id.to_string());
            }
        }
        let id = ids::new_id("page");
        let mut page = store::Page::new(&id, topic, "summary", "summaries", body);
        page.sources = sources;
        store::save(&self.root, &page)?;
        let mut state = self.write_state();
        state.pages.push(page.clone());
        self.sync_file(&mut state, &page.path)?;
        Ok(id)
    }

    /// `forget(id, reason) -> Tombstone` — tombstone a page: frontmatter
    /// `forgotten: true` + `forgotten_reason` + `forgotten_at`. The content
    /// is NEVER deleted. Forgetting an already-forgotten page is a no-op
    /// that returns the existing tombstone; an unknown id is an error.
    pub fn forget(&self, id: &str, reason: &str) -> Result<Tombstone> {
        let id = id.trim();
        if id.is_empty() {
            return Err(Error::Memory("page id must not be empty".into()));
        }
        let mut state = self.write_state();
        let idx = state
            .pages
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| Error::Memory(format!("unknown page id: {id}")))?;
        if state.pages[idx].forgotten {
            let page = &state.pages[idx];
            return Ok(Tombstone {
                id: page.id.clone(),
                reason: page.forgotten_reason.clone().unwrap_or_default(),
                at: page.forgotten_at.clone().unwrap_or_else(|| ids::now_iso()),
            });
        }
        let now = ids::now_iso();
        {
            let page = &mut state.pages[idx];
            page.forgotten = true;
            page.forgotten_reason = Some(store::single_line(reason));
            page.forgotten_at = Some(now.clone());
        }
        let rel = state.pages[idx].path.clone();
        store::save(&self.root, &state.pages[idx])?;
        self.sync_file(&mut state, &rel)?;
        Ok(Tombstone {
            id: id.to_string(),
            reason: reason.to_string(),
            at: now,
        })
    }

    /// Number of pages that are not forgotten.
    pub fn page_count(&self) -> usize {
        self.read_state()
            .pages
            .iter()
            .filter(|p| !p.forgotten)
            .count()
    }

    /// List every page (including tombstoned ones), newest first.
    pub fn list_pages(&self) -> Vec<PageSummary> {
        let state = self.read_state();
        let mut summaries: Vec<PageSummary> = state
            .pages
            .iter()
            .map(|p| PageSummary {
                id: p.id.clone(),
                title: p.title.clone(),
                kind: p.kind.clone(),
                forgotten: p.forgotten,
                updated_at: p.updated.clone(),
            })
            .collect();
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        summaries
    }

    /// Re-hash every file under the page directories, report drift against
    /// `manifest.json` (`missing:` / `changed:` / `untracked:`), then update
    /// the manifest to the current disk state. Returns the drift messages.
    pub fn verify(&self) -> Result<Vec<String>> {
        let disk = manifest::Manifest::scan_disk(&self.root)?;
        let mut state = self.write_state();
        let drift = state.manifest.diff(&disk);
        state.manifest.map = disk;
        state.manifest.save(&self.root)?;
        Ok(drift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Unique temp root per call so tests never collide, even in parallel.
    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "astrid-mem-{tag}-{}-{nanos}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remember_recall_roundtrip_finds_the_page_first() {
        let root = temp_root("roundtrip");
        let kernel = MemoryKernel::open(&root).unwrap();
        assert_eq!(kernel.page_count(), 0);

        let id = kernel
            .remember(
                "Astrid turn loop",
                "The turn loop is bounded at max_turn_steps = 8 and every step publishes events.",
                "note",
            )
            .unwrap();
        assert!(id.starts_with("page_"));

        kernel
            .remember(
                "Unrelated grocery list",
                "milk, eggs, coffee beans for the espresso machine",
                "log",
            )
            .unwrap();

        let hits = kernel.recall("turn loop max_turn_steps", 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, id, "the remembered page must rank first");
        assert!(hits[0].snippet.contains("turn loop"));
        // per-stage scores present
        let stage_names: Vec<&str> = hits[0].stages.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(stage_names, vec!["bm25", "recency", "phrase"]);
        assert!(hits[0].score > 0.0);
        cleanup(&root);
    }

    #[test]
    fn recall_ranks_exact_phrase_above_weak_bm25_match() {
        let root = temp_root("phrase");
        let kernel = MemoryKernel::open(&root).unwrap();
        let strong = kernel
            .remember(
                "Quantum flux calibration",
                "The zeta quantum flux must be recalibrated after every kernel panic because the zeta quantum flux drifts.",
                "note",
            )
            .unwrap();
        kernel
            .remember(
                "Kitchen notes",
                "the flux capacitor in the microwave is decorative only, also buy bread",
                "note",
            )
            .unwrap();
        kernel
            .remember(
                "Gardening",
                "roses need water; tomatoes need sun; nothing about physics here",
                "note",
            )
            .unwrap();

        let hits = kernel.recall("zeta quantum flux", 3).unwrap();
        assert_eq!(
            hits[0].id, strong,
            "exact-phrase page must outrank weak matches"
        );
        let strong_hit = &hits[0];
        let phrase = strong_hit
            .stages
            .iter()
            .find(|(n, _)| n == "phrase")
            .unwrap()
            .1;
        assert_eq!(phrase, 1.0);
        cleanup(&root);
    }

    #[test]
    fn forget_tombstones_and_hides_from_recall_and_count() {
        let root = temp_root("forget");
        let kernel = MemoryKernel::open(&root).unwrap();
        let id = kernel
            .remember("Secret plan", "we launch the rockets at dawn", "note")
            .unwrap();
        kernel
            .remember("Other plan", "we water the plants at noon", "log")
            .unwrap();
        assert_eq!(kernel.page_count(), 2);

        let tomb = kernel
            .forget(&id, "superseded by the slow launch plan")
            .unwrap();
        assert_eq!(tomb.id, id);
        assert_eq!(tomb.reason, "superseded by the slow launch plan");
        assert!(!tomb.at.is_empty());

        assert_eq!(kernel.page_count(), 1, "tombstoned pages leave page_count");
        let hits = kernel.recall("rockets at dawn", 10).unwrap();
        assert!(
            hits.iter().all(|h| h.id != id),
            "tombstoned pages leave recall"
        );

        // summary listing still shows the tombstone (content never deleted)
        let summary = kernel
            .list_pages()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        assert!(summary.forgotten);

        // the file on disk still carries the content, now tombstoned
        let pages = store::load_pages(&root).unwrap();
        let page = pages.iter().find(|p| p.id == id).unwrap();
        assert!(page.forgotten);
        assert!(page.body.contains("rockets"));
        assert_eq!(
            page.forgotten_reason.as_deref(),
            Some("superseded by the slow launch plan")
        );

        // double-forget is an idempotent no-op returning the tombstone
        let again = kernel.forget(&id, "again").unwrap();
        assert_eq!(again.reason, "superseded by the slow launch plan");

        // unknown id is an error
        assert!(kernel.forget("page_doesnotexist", "why").is_err());
        cleanup(&root);
    }

    #[test]
    fn remember_rejects_unknown_kind() {
        let root = temp_root("kinds");
        let kernel = MemoryKernel::open(&root).unwrap();
        assert!(kernel.remember("t", "b", "diary").is_err());
        assert!(kernel.remember("t", "b", "note").is_ok());
        assert!(kernel.remember("t", "b", "reference").is_ok());
        assert!(kernel.remember("t", "b", "log").is_ok());
        assert!(
            kernel.remember("", "b", "note").is_err(),
            "empty topic rejected"
        );
        cleanup(&root);
    }

    #[test]
    fn entity_matches_exact_name_and_alias() {
        let root = temp_root("entity");
        // First open just creates the directory skeleton.
        MemoryKernel::open(&root).unwrap();

        // An entity page under entities/ (kind "entity", with aliases+edges).
        let mut entity = store::Page::new(
            "page_1111111111111111aaaaaaaaaaaaaaaa",
            "Quill",
            "entity",
            "entities",
            "Quill reviews code and leaves precise, kind comments.",
        );
        entity.aliases = vec!["the reviewer".into(), "Q".into()];
        entity.edges = vec![
            ("reviews_for".into(), "sable".into()),
            ("owns".into(), "style guide".into()),
        ];
        store::save(&root, &entity).unwrap();

        // Fresh open picks the external file up (sync hashes it in).
        let kernel = MemoryKernel::open(&root).unwrap();

        let by_name = kernel.entity("Quill").unwrap().expect("exact name match");
        assert_eq!(by_name.id, entity.id);
        assert_eq!(by_name.name, "Quill");
        assert_eq!(by_name.kind, "entity");
        assert_eq!(by_name.aliases.len(), 2);

        let by_alias = kernel.entity("the reviewer").unwrap().expect("alias match");
        assert_eq!(by_alias.id, entity.id);

        let edges: Vec<(String, String)> = by_name.edges.clone();
        assert!(edges.contains(&("reviews_for".to_string(), "sable".to_string())));

        assert!(kernel.entity("Nobody").unwrap().is_none());
        assert!(kernel.entity("").unwrap().is_none());
        cleanup(&root);
    }

    #[test]
    fn synthesize_links_sources_in_frontmatter() {
        let root = temp_root("synthesize");
        let kernel = MemoryKernel::open(&root).unwrap();
        let a = kernel
            .remember("Design A", "the kernel gate classifies input", "note")
            .unwrap();
        let b = kernel
            .remember("Design B", "the turn loop is bounded", "note")
            .unwrap();

        let summary_id = kernel
            .synthesize(
                "Design review",
                &[a.clone(), b.clone(), a.clone()],
                "Both designs share the five-verb memory surface; nothing was found missing.",
            )
            .unwrap();
        assert!(summary_id.starts_with("page_"));

        let pages = store::load_pages(&root).unwrap();
        let summary = pages.iter().find(|p| p.id == summary_id).unwrap();
        assert_eq!(summary.kind, "summary");
        assert!(summary.path.starts_with("summaries/"));
        assert_eq!(summary.sources, vec![a, b], "sources deduplicated in order");

        // and the summary itself is findable through recall
        let hits = kernel.recall("five-verb memory surface", 5).unwrap();
        assert_eq!(hits[0].id, summary_id);
        cleanup(&root);
    }

    #[test]
    fn verify_detects_external_file_modification() {
        let root = temp_root("verify");
        let kernel = MemoryKernel::open(&root).unwrap();
        let id = kernel
            .remember("Ledger", "amounts must balance", "note")
            .unwrap();
        assert!(
            kernel.verify().unwrap().is_empty(),
            "clean store has no drift"
        );

        // External mutation behind the kernel's back.
        let pages = store::load_pages(&root).unwrap();
        let rel = pages.iter().find(|p| p.id == id).unwrap().path.clone();
        let file = root.join(&rel);
        let original = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, format!("{original}\nExternally tampered.")).unwrap();

        let drift = kernel.verify().unwrap();
        assert_eq!(drift.len(), 1, "exactly one drift message: {drift:?}");
        assert!(drift[0].starts_with("changed: "));
        assert!(drift[0].contains(&rel));

        // Manifest was updated to disk state: second verify is clean.
        assert!(kernel.verify().unwrap().is_empty());

        // A deleted file is reported as missing.
        std::fs::remove_file(&file).unwrap();
        let drift = kernel.verify().unwrap();
        assert!(
            drift.iter().any(|d| d.starts_with("missing: ")),
            "{drift:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn protocol_version_is_exposed() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
        assert_eq!(protocol::PROTOCOL_VERSION, PROTOCOL_VERSION);
    }

    #[test]
    fn empty_query_returns_newest_k_fallback() {
        let root = temp_root("newest");
        let kernel = MemoryKernel::open(&root).unwrap();
        for i in 0..4 {
            kernel
                .remember(&format!("Page {i}"), &format!("body of page {i}"), "log")
                .unwrap();
        }
        let hits = kernel.recall("", 2).unwrap();
        assert_eq!(hits.len(), 2);
        let all = kernel.recall("   ", 10).unwrap();
        assert_eq!(
            all.len(),
            4,
            "tokenless query falls back to all pages, newest first"
        );
        // k = 0 asks for nothing
        assert!(kernel.recall("anything", 0).unwrap().is_empty());
        cleanup(&root);
    }

    #[test]
    fn reopen_syncs_new_files_and_persists_pages() {
        let root = temp_root("reopen");
        let id = {
            let kernel = MemoryKernel::open(&root).unwrap();
            let id = kernel
                .remember("Persistent", "state survives reopening", "note")
                .unwrap();
            id
        };
        // Drop a foreign page file on disk, then reopen.
        std::fs::write(
            root.join("notes/dropped-99999999.md"),
            "---\nid: page_dropped9999999999deadbeef\nkind: reference\ntitle: Dropped note\ncreated: 2026-01-01T00:00:00Z\nupdated: 2026-01-01T00:00:00Z\nforgotten: false\n---\n\nWritten by an external tool.\n",
        )
        .unwrap();

        let kernel = MemoryKernel::open(&root).unwrap();
        assert_eq!(kernel.page_count(), 2);
        let summary = kernel
            .list_pages()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        assert_eq!(summary.title, "Persistent");

        // Manifest picked up the dropped file (untracked → synced on open).
        let manifest = manifest::Manifest::load(&root).unwrap();
        assert!(manifest.map.contains_key("notes/dropped-99999999.md"));
        cleanup(&root);
    }

    #[test]
    fn search_stage_scores_are_reported() {
        let root = temp_root("stages");
        let kernel = MemoryKernel::open(&root).unwrap();
        kernel
            .remember(
                "Bm25 stage",
                "the bm25 stage scores documents by term frequency saturation",
                "note",
            )
            .unwrap();
        let hits = kernel.recall("bm25 stage", 1).unwrap();
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.stages.len(), 3);
        assert!(hit.stages.iter().any(|(n, v)| n == "bm25" && *v > 0.0));
        assert!(hit.stages.iter().any(|(n, v)| n == "phrase" && *v == 1.0));
        assert!(hit.stages.iter().any(|(n, v)| n == "recency" && *v > 0.0));
        cleanup(&root);
    }
}
