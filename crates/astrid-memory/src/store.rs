//! Markdown page store: `---` fenced frontmatter + body.
//!
//! This module owns load/parse/save of memory pages. Page files live under
//! `<root>/{notes,entities,summaries}/<slug>-<id8>.md` and carry all metadata
//! in the frontmatter; the body is plain markdown. The id inside the
//! frontmatter is authoritative — the filename is a convenience.

use astrid_core::{ids, Error, Result};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Subdirectories that hold memory pages, in manifest/load order.
pub const PAGE_DIRS: &[&str] = &["notes", "entities", "summaries"];

/// Maximum slug length used in page filenames.
const SLUG_MAX: usize = 48;

/// One memory page, parsed from (or bound for) a markdown file.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Authoritative page id (`page_<uuid>`), matches the frontmatter.
    pub id: String,
    pub title: String,
    /// `note` | `reference` | `log` | `entity` | `summary`.
    pub kind: String,
    pub created: String,
    pub updated: String,
    pub forgotten: bool,
    pub aliases: Vec<String>,
    /// `(relation, target)` pairs, e.g. `("owner", "sable")`.
    pub edges: Vec<(String, String)>,
    /// Source page ids linked by a summary page.
    pub sources: Vec<String>,
    pub forgotten_reason: Option<String>,
    pub forgotten_at: Option<String>,
    /// Markdown body (everything after the closing fence).
    pub body: String,
    /// Root-relative path with forward slashes, e.g. `notes/foo-1234abcd.md`.
    pub path: String,
}

impl Page {
    /// A brand-new page stamped with the current time.
    pub fn new(id: &str, title: &str, kind: &str, dir: &str, body: &str) -> Self {
        let now = ids::now_iso();
        let uuid = id.strip_prefix("page_").unwrap_or(id);
        let slug = slugify(title);
        Self {
            id: id.to_string(),
            title: title.to_string(),
            kind: kind.to_string(),
            created: now.clone(),
            updated: now,
            forgotten: false,
            aliases: Vec::new(),
            edges: Vec::new(),
            sources: Vec::new(),
            forgotten_reason: None,
            forgotten_at: None,
            body: body.to_string(),
            path: format!("{dir}/{slug}-{}.md", &uuid[..uuid.len().min(8)]),
        }
    }
}

/// Slugify a title for use in a filename: lowercase ascii alnum runs joined
/// by `-`, collapsed, trimmed, capped at 48 chars. Never returns empty.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = true; // suppress leading dashes
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > SLUG_MAX {
        out.truncate(SLUG_MAX);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        "page".to_string()
    } else {
        out
    }
}

/// Flatten a string to a single line so it is safe inside frontmatter.
pub fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].trim().to_string()
    } else {
        v.to_string()
    }
}

fn comma_list(value: Option<&String>) -> Vec<String> {
    value
        .map(|s| {
            s.split(',')
                .map(|p| unquote(p))
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Render a page to its on-disk markdown form.
pub fn render(page: &Page) -> String {
    let mut s = String::from("---\n");
    s.push_str(&format!("id: {}\n", page.id));
    s.push_str(&format!("title: {}\n", single_line(&page.title)));
    s.push_str(&format!("kind: {}\n", page.kind));
    s.push_str(&format!("created: {}\n", page.created));
    s.push_str(&format!("updated: {}\n", page.updated));
    s.push_str(&format!("forgotten: {}\n", page.forgotten));
    if !page.aliases.is_empty() {
        s.push_str(&format!("aliases: {}\n", page.aliases.join(", ")));
    }
    if !page.edges.is_empty() {
        let pairs: Vec<String> = page
            .edges
            .iter()
            .map(|(rel, target)| format!("{}: {}", single_line(rel), single_line(target)))
            .collect();
        s.push_str(&format!("edges: {}\n", pairs.join(", ")));
    }
    if !page.sources.is_empty() {
        s.push_str(&format!("sources: {}\n", page.sources.join(", ")));
    }
    if let Some(reason) = &page.forgotten_reason {
        s.push_str(&format!("forgotten_reason: {}\n", single_line(reason)));
    }
    if let Some(at) = &page.forgotten_at {
        s.push_str(&format!("forgotten_at: {}\n", at));
    }
    s.push_str("---\n");
    s.push_str(page.body.trim_start_matches('\n'));
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Parse a page from markdown text. The frontmatter must open on line 1 and
/// close with a second `---` fence; everything after it is the body.
pub fn parse(text: &str) -> Result<Page> {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Err(Error::Memory(
            "page is missing the frontmatter fence".into(),
        ));
    }

    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_frontmatter = true;
    for line in lines {
        if in_frontmatter {
            if line.trim_end() == "---" {
                in_frontmatter = false;
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                return Err(Error::Memory(format!("invalid frontmatter line: {line:?}")));
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(Error::Memory(format!(
                    "frontmatter key missing on line: {line:?}"
                )));
            }
            map.insert(key.to_string(), unquote(value));
        } else {
            body_lines.push(line);
        }
    }
    if in_frontmatter {
        return Err(Error::Memory("frontmatter fence is never closed".into()));
    }

    let missing = |field: &str| Error::Memory(format!("frontmatter is missing `{field}`"));
    let id = map
        .get("id")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing("id"))?
        .clone();
    let title = map
        .get("title")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing("title"))?
        .clone();
    let kind = map
        .get("kind")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing("kind"))?
        .clone();
    let now = ids::now_iso();
    let created = map
        .get("created")
        .filter(|v| !v.is_empty())
        .unwrap_or(&now)
        .clone();
    let updated = map
        .get("updated")
        .filter(|v| !v.is_empty())
        .unwrap_or(&now)
        .clone();
    let forgotten = map
        .get("forgotten")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut edges = Vec::new();
    for pair in comma_list(map.get("edges")) {
        if let Some((rel, target)) = pair.split_once(':') {
            let (rel, target) = (rel.trim(), target.trim());
            if !rel.is_empty() && !target.is_empty() {
                edges.push((rel.to_string(), target.to_string()));
            }
        }
    }

    let opt = |key: &str| {
        map.get(key)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    Ok(Page {
        id,
        title,
        kind,
        created,
        updated,
        forgotten,
        aliases: comma_list(map.get("aliases")),
        edges,
        sources: comma_list(map.get("sources")),
        forgotten_reason: opt("forgotten_reason"),
        forgotten_at: opt("forgotten_at"),
        body: body_lines.join("\n"),
        path: String::new(),
    })
}

/// Root-relative path with forward slashes.
pub fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Render and write a page to `root/<page.path>`.
pub fn save(root: &Path, page: &Page) -> Result<()> {
    let path = root.join(&page.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, render(page))?;
    Ok(())
}

/// Load every parsable `.md` page under `root/{notes,entities,summaries}`.
/// Unparsable files are skipped with a warning — memory is never bricked by
/// one bad file. Returned sorted by id for determinism.
pub fn load_pages(root: &Path) -> Result<Vec<Page>> {
    let mut pages = Vec::new();
    for dir in PAGE_DIRS {
        let dir_path = root.join(dir);
        let entries = match std::fs::read_dir(&dir_path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && p.extension().map(|x| x == "md").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            let rel = rel_path(root, &path);
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse(&text) {
                    Ok(mut page) => {
                        page.path = rel;
                        pages.push(page);
                    }
                    Err(e) => tracing::warn!("memory: skipping unparsable page `{rel}`: {e}"),
                },
                Err(e) => tracing::warn!("memory: skipping unreadable page `{rel}`: {e}"),
            }
        }
    }
    pages.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_page() -> Page {
        let mut page = Page::new(
            "page_0123456789abcdef0123456789abcdef",
            "Atlas Notes",
            "note",
            "notes",
            "hello\nworld",
        );
        page.path = "notes/atlas-notes-01234567.md".into();
        page
    }

    #[test]
    fn slugify_collapses_and_caps() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  --  Weird___  Title  --"), "weird-title");
        assert_eq!(slugify("!!!"), "page");
        let long = slugify(&"x".repeat(100));
        assert_eq!(long.len(), SLUG_MAX);
        assert_eq!(slugify("A B  C"), "a-b-c");
    }

    #[test]
    fn render_parse_roundtrip_preserves_all_fields() {
        let mut page = sample_page();
        page.aliases = vec!["atlas alias".into(), "synth".into()];
        page.edges = vec![
            ("depends_on".into(), "sable".into()),
            ("reviewed_by".into(), "quill".into()),
        ];
        page.sources = vec!["page_aaaa1111".into(), "page_bbbb2222".into()];
        page.body = "# Heading\n\n- one\n- two".into();

        let rendered = render(&page);
        let parsed = parse(&rendered).expect("parses");
        assert_eq!(parsed.id, page.id);
        assert_eq!(parsed.title, page.title);
        assert_eq!(parsed.kind, page.kind);
        assert_eq!(parsed.created, page.created);
        assert_eq!(parsed.updated, page.updated);
        assert!(!parsed.forgotten);
        assert_eq!(parsed.aliases, page.aliases);
        assert_eq!(parsed.edges, page.edges);
        assert_eq!(parsed.sources, page.sources);
        assert_eq!(parsed.body, page.body);
        assert_eq!(parsed.forgotten_reason, None);
        // idempotent render
        assert_eq!(render(&parsed), rendered);
    }

    #[test]
    fn render_parse_roundtrip_preserves_tombstone() {
        let mut page = sample_page();
        page.forgotten = true;
        page.forgotten_reason = Some("superseded by summaries/merge-99.md".into());
        page.forgotten_at = Some("2026-11-10T00:00:00Z".into());

        let parsed = parse(&render(&page)).expect("parses");
        assert!(parsed.forgotten);
        assert_eq!(
            parsed.forgotten_reason.as_deref(),
            Some("superseded by summaries/merge-99.md")
        );
        assert_eq!(parsed.forgotten_at.as_deref(), Some("2026-11-10T00:00:00Z"));
    }

    #[test]
    fn parse_rejects_bad_frontmatter() {
        assert!(parse("no fence here").is_err());
        assert!(parse("---\nid: page_x\n").is_err(), "unclosed fence");
        assert!(parse("---\nnot-a-pair\n---\nbody").is_err());
        let no_id = "---\ntitle: T\nkind: note\n---\nbody";
        assert!(parse(no_id).is_err());
        let quoted = parse("---\nid: \"page_q\"\ntitle: 'Quoted Title'\nkind: note\n---\nbody")
            .expect("quoted frontmatter parses");
        assert_eq!(quoted.id, "page_q");
        assert_eq!(quoted.title, "Quoted Title");
    }
}
