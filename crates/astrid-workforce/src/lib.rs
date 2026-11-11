//! astrid-workforce — curated agent roster, divisions, skills discovery.
//!
//! The roster is curated, not generated: 12 agents across 5 divisions, each
//! declaring its capabilities explicitly (see [`roster`]). [`skills`]
//! discovers `SKILL.md` files — bundled at rank 600, project skills under
//! `<data_dir>/skills/` at rank 100 overriding same-name bundled skills —
//! and exposes progressive disclosure: metadata listing, then body fetch.

pub mod roster;
pub mod skills;

pub use roster::{AgentSpec, Division, Roster, ROSTER_TOML};
pub use skills::{parse_frontmatter, SkillIndex, SkillMeta};
