//! SKILL.md discovery with progressive disclosure.
//!
//! Eight skills are compiled into the binary; a project may add more under
//! `<data_dir>/skills/<name>/SKILL.md`, which **overrides** a bundled skill
//! of the same name (project rank 100 beats bundled rank 600). Invalid
//! `SKILL.md` files are skipped with a `tracing` warning — discovery is
//! never fatal. `list()` gives metadata; `body()` fetches one full markdown
//! body on demand.
//!
//! Frontmatter is parsed by [`parse_frontmatter`], a hand-rolled YAML-subset
//! parser: a `---` opening fence on line 1, `key: value` lines (values may
//! be quoted), a closing `---` fence. That is the only subset the skills
//! use, so the parser stays small and honest.

use astrid_core::{Error, Result};
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Name of the skill manifest file inside a skill directory.
pub const SKILL_FILE: &str = "SKILL.md";

/// Source rank of project skills (override bundled ones).
pub const PROJECT_SKILL_RANK: u32 = 100;

/// Source rank of bundled skills.
pub const BUNDLED_SKILL_RANK: u32 = 600;

/// Kebab-case skill names: `^[a-z0-9]+(-[a-z0-9]+)*$`.
fn is_kebab_case(name: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("valid regex"))
        .is_match(name)
}

/// Strip one layer of matching quotes from a frontmatter value.
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

/// Hand-rolled YAML-subset frontmatter parser.
///
/// The text must open with a `---` fence on the first line and close with a
/// second `---` fence; between them, only `key: value` pairs (blank lines
/// are skipped; a line without a colon is an error → `None`). Values are
/// trimmed and stripped of one layer of matching quotes. Returns the parsed
/// map plus the body slice — everything after the closing fence, with
/// leading blank lines stripped — or `None` if no valid fence exists.
pub fn parse_frontmatter(text: &str) -> Option<(BTreeMap<String, String>, &str)> {
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut map = BTreeMap::new();
    let mut pos = 0usize;
    loop {
        if pos >= rest.len() {
            return None; // fence never closed
        }
        let line_end = rest[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(rest.len());
        let line = rest[pos..line_end].trim_end_matches('\r');
        pos = if line_end < rest.len() {
            line_end + 1
        } else {
            line_end
        };

        if line.trim() == "---" {
            let body = rest[pos..].trim_start_matches(['\r', '\n']);
            return Some((map, body));
        }
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':')?;
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        map.insert(key.to_string(), unquote(value));
    }
}

/// Metadata for one discovered skill. `path` is `None` for compiled-in
/// skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub source_rank: u32,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct SkillEntry {
    meta: SkillMeta,
    body: String,
}

/// The eight compiled-in skills: `(expected name, raw SKILL.md)`.
const BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "systematic-debugging",
        include_str!("../assets/skills/systematic-debugging/SKILL.md"),
    ),
    (
        "tdd-loop",
        include_str!("../assets/skills/tdd-loop/SKILL.md"),
    ),
    (
        "verification-before-completion",
        include_str!("../assets/skills/verification-before-completion/SKILL.md"),
    ),
    (
        "code-review-checklist",
        include_str!("../assets/skills/code-review-checklist/SKILL.md"),
    ),
    (
        "repo-onboarding",
        include_str!("../assets/skills/repo-onboarding/SKILL.md"),
    ),
    (
        "writing-clear-docs",
        include_str!("../assets/skills/writing-clear-docs/SKILL.md"),
    ),
    (
        "incident-response",
        include_str!("../assets/skills/incident-response/SKILL.md"),
    ),
    (
        "memory-hygiene",
        include_str!("../assets/skills/memory-hygiene/SKILL.md"),
    ),
];

fn parse_skill(
    raw: &str,
    path: Option<&Path>,
    rank: u32,
) -> std::result::Result<SkillEntry, String> {
    let Some((frontmatter, body)) = parse_frontmatter(raw) else {
        return Err("no valid frontmatter fence".to_string());
    };
    let missing = |field: &str| format!("frontmatter is missing `{field}`");
    let name = frontmatter
        .get("name")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing("name"))?;
    let description = frontmatter
        .get("description")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| missing("description"))?;
    if !is_kebab_case(name) {
        return Err(format!("name `{name}` is not kebab-case"));
    }
    Ok(SkillEntry {
        meta: SkillMeta {
            name: name.clone(),
            description: description.clone(),
            when_to_use: frontmatter
                .get("when_to_use")
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            source_rank: rank,
            path: path.map(Path::to_path_buf),
        },
        body: body.to_string(),
    })
}

/// An index of discovered skills: bundled plus project, with project
/// overrides applied. Names are unique; `list()` is ordered by rank then
/// name.
#[derive(Debug, Clone)]
pub struct SkillIndex {
    entries: Vec<SkillEntry>,
}

impl SkillIndex {
    /// Discover skills. `project_dir` is `<data_dir>/skills`; each immediate
    /// subdirectory may contain a `SKILL.md`. Bundled skills are always
    /// present; a project skill replaces the bundled skill of the same name.
    /// IO errors on the project directory itself are returned; individual
    /// bad skill files are skipped with a warning, never fatal.
    pub fn load(project_dir: Option<&Path>) -> Result<Self> {
        let mut entries = Vec::new();
        for (expected, raw) in BUNDLED_SKILLS {
            match parse_skill(raw, None, BUNDLED_SKILL_RANK) {
                Ok(entry) => entries.push(entry),
                Err(reason) => {
                    // The bundled set is known-good; this can only be a build bug.
                    tracing::error!("workforce: bundled skill `{expected}` invalid: {reason}");
                }
            }
        }
        if let Some(dir) = project_dir {
            load_project_skills(dir, &mut entries)?;
        }

        // Project skills override same-name bundled skills: group by name
        // (name, rank, path) and keep the first entry per name — the lowest
        // rank wins. Then restore the rank-then-name display order.
        entries.sort_by(|a, b| {
            a.meta
                .name
                .cmp(&b.meta.name)
                .then(a.meta.source_rank.cmp(&b.meta.source_rank))
                .then(a.meta.path.cmp(&b.meta.path))
        });
        entries.dedup_by(|a, b| a.meta.name == b.meta.name);
        entries.sort_by(|a, b| {
            a.meta
                .source_rank
                .cmp(&b.meta.source_rank)
                .then(a.meta.name.cmp(&b.meta.name))
        });
        Ok(Self { entries })
    }

    /// Metadata for every skill, sorted by rank then name.
    pub fn list(&self) -> Vec<SkillMeta> {
        self.entries.iter().map(|e| e.meta.clone()).collect()
    }

    /// The full markdown body of one skill (progressive disclosure:
    /// `list` first, load bodies on demand). Unknown names are errors.
    pub fn body(&self, name: &str) -> Result<String> {
        match self.entries.iter().find(|e| e.meta.name == name) {
            Some(entry) => Ok(entry.body.clone()),
            None => Err(Error::Other(format!("unknown skill: {name}"))),
        }
    }
}

fn load_project_skills(dir: &Path, entries: &mut Vec<SkillEntry>) -> Result<()> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut paths: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    paths.sort();
    for path in paths {
        let skill_file = path.join(SKILL_FILE);
        if !skill_file.is_file() {
            continue;
        }
        let raw = match std::fs::read_to_string(&skill_file) {
            Ok(raw) => raw,
            Err(e) => {
                tracing::warn!(
                    "workforce: skipping unreadable project skill `{}`: {e}",
                    skill_file.display()
                );
                continue;
            }
        };
        match parse_skill(&raw, Some(&path), PROJECT_SKILL_RANK) {
            Ok(entry) => entries.push(entry),
            Err(reason) => {
                tracing::warn!(
                    "workforce: skipping invalid project skill `{}`: {reason}",
                    skill_file.display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_valid_and_quoted() {
        let text = "---\nname: my-skill\ndescription: \"Does things: fast\"\nwhen_to_use: 'Always'\n---\n\n# Body\n\nline two\n";
        let (map, body) = parse_frontmatter(text).expect("valid frontmatter");
        assert_eq!(map.get("name").unwrap(), "my-skill");
        assert_eq!(map.get("description").unwrap(), "Does things: fast");
        assert_eq!(map.get("when_to_use").unwrap(), "Always");
        assert_eq!(body, "# Body\n\nline two\n");

        // no opening fence / no closing fence / missing colon
        assert!(parse_frontmatter("name: x\n").is_none());
        assert!(parse_frontmatter("---\nname: x\n").is_none());
        assert!(parse_frontmatter("---\nno colon here\n---\n").is_none());
        assert!(parse_frontmatter("---\n: novalue\n---\n").is_none());
        assert!(
            parse_frontmatter("  ---\nname: x\n---\n").is_none(),
            "fence must be on line 1"
        );
        // empty frontmatter is structurally valid
        let (map, body) = parse_frontmatter("---\n---\nbody only").unwrap();
        assert!(map.is_empty());
        assert_eq!(body, "body only");
    }

    #[test]
    fn bundled_index_lists_at_least_8_skills() {
        let index = SkillIndex::load(None).unwrap();
        let list = index.list();
        assert!(
            list.len() >= 8,
            "expected >=8 bundled skills, got {}",
            list.len()
        );
        assert_eq!(list.len(), BUNDLED_SKILLS.len());
        for meta in &list {
            assert_eq!(meta.source_rank, BUNDLED_SKILL_RANK);
            assert!(meta.path.is_none(), "bundled skills have no path");
            assert!(is_kebab_case(&meta.name), "`{}` not kebab-case", meta.name);
            assert!(!meta.description.is_empty());
        }
        let names: Vec<&str> = list.iter().map(|m| m.name.as_str()).collect();
        for expected in [
            "systematic-debugging",
            "tdd-loop",
            "verification-before-completion",
            "code-review-checklist",
            "repo-onboarding",
            "writing-clear-docs",
            "incident-response",
            "memory-hygiene",
        ] {
            assert!(
                names.contains(&expected),
                "missing bundled skill {expected}"
            );
        }
        // sorted by rank then name
        let mut sorted = list.clone();
        sorted.sort_by(|a, b| a.source_rank.cmp(&b.source_rank).then(a.name.cmp(&b.name)));
        assert_eq!(list, sorted);
    }

    #[test]
    fn body_roundtrip_and_unknown_error() {
        let index = SkillIndex::load(None).unwrap();
        let body = index.body("tdd-loop").unwrap();
        assert!(body.starts_with("# TDD loop"));
        assert!(body.contains("Red: write one small failing test"));
        let body = index.body("memory-hygiene").unwrap();
        assert!(body.contains("Tombstone, never delete"));
        let err = index.body("no-such-skill").unwrap_err();
        assert!(matches!(err, Error::Other(ref m) if m.contains("unknown skill: no-such-skill")));
    }

    #[test]
    fn project_skills_override_bundled_and_invalid_is_skipped() {
        let root = std::env::temp_dir().join(format!(
            "astrid-skills-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project = root.join("skills");
        std::fs::create_dir_all(project.join("tdd-loop")).unwrap();
        std::fs::write(
            project.join("tdd-loop").join("SKILL.md"),
            "---\nname: tdd-loop\ndescription: Project-local variant of the TDD loop.\n---\n\n# Project TDD\n\nhouse rules go here\n",
        )
        .unwrap();
        // one valid extra project skill
        std::fs::create_dir_all(project.join("house-style")).unwrap();
        std::fs::write(
            project.join("house-style").join("SKILL.md"),
            "---\nname: house-style\ndescription: Our local style rules.\nwhen_to_use: Use on every PR.\n---\n\n# House style\n\n1. No unwraps.\n",
        )
        .unwrap();
        // one invalid project skill: missing description
        std::fs::create_dir_all(project.join("broken-skill")).unwrap();
        std::fs::write(
            project.join("broken-skill").join("SKILL.md"),
            "---\nname: broken-skill\n---\n\nno description here\n",
        )
        .unwrap();
        // one invalid: bad name (not kebab-case)
        std::fs::create_dir_all(project.join("BadName")).unwrap();
        std::fs::write(
            project.join("BadName").join("SKILL.md"),
            "---\nname: BadName\ndescription: Nope.\n---\n\nbody\n",
        )
        .unwrap();
        // a dir without SKILL.md is just ignored
        std::fs::create_dir_all(project.join("empty-dir")).unwrap();

        let index = SkillIndex::load(Some(&project)).unwrap();
        let list = index.list();
        let by_name = |name: &str| list.iter().find(|m| m.name == name).cloned();

        // project override wins over bundled
        let tdd = by_name("tdd-loop").expect("tdd-loop present");
        assert_eq!(tdd.source_rank, PROJECT_SKILL_RANK);
        assert_eq!(tdd.path, Some(project.join("tdd-loop")));
        assert!(index.body("tdd-loop").unwrap().starts_with("# Project TDD"));

        // extra project skill discovered with its when_to_use
        let house = by_name("house-style").expect("house-style present");
        assert_eq!(house.when_to_use.as_deref(), Some("Use on every PR."));

        // invalid skills skipped, never fatal
        assert!(by_name("broken-skill").is_none());
        assert!(by_name("BadName").is_none());
        assert_eq!(
            list.len(),
            BUNDLED_SKILLS.len() + 1,
            "one extra skill beyond the 8 bundled"
        );

        // missing project dir is not an error
        let missing = root.join("does-not-exist");
        assert!(SkillIndex::load(Some(&missing)).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kebab_case_rules() {
        assert!(is_kebab_case("tdd-loop"));
        assert!(is_kebab_case("a"));
        assert!(is_kebab_case("verification-before-completion"));
        assert!(!is_kebab_case("TDD-Loop"));
        assert!(!is_kebab_case("tdd_loop"));
        assert!(!is_kebab_case("tdd-"));
        assert!(!is_kebab_case("-tdd"));
        assert!(!is_kebab_case(""));
        assert!(!is_kebab_case("tdd--loop"));
    }
}
