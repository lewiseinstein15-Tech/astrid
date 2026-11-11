//! The curated agent roster: 12 agents across 5 divisions, embedded as TOML.
//!
//! The roster is **curated, not generated** (ARCHITECTURE.md §5): every agent
//! declares its capabilities explicitly. [`Roster::bundled`] compiles the
//! known-good TOML into the binary and unwraps it — a broken embedded file is
//! a build-time bug, not a runtime condition — while [`Roster::from_toml`]
//! returns a `Result` so callers and tests can validate arbitrary TOML.

use astrid_core::{Error, Result};
use serde::Deserialize;

/// The embedded roster source (compiled in, never read from disk).
pub const ROSTER_TOML: &str = include_str!("../assets/roster.toml");

/// A division: a named group of agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Division {
    pub id: String,
    pub label: String,
    pub description: String,
}

/// One curated agent and its declared capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    /// Division id; must exist in the roster's divisions.
    pub division: String,
    pub role: String,
    pub capabilities: Vec<String>,
    /// First-person system prompt with the agent's distinct voice.
    pub system_prompt: String,
}

#[derive(Debug, Deserialize)]
struct RawRoster {
    #[serde(default)]
    division: Vec<RawDivision>,
    #[serde(default)]
    agent: Vec<RawAgent>,
}

#[derive(Debug, Deserialize)]
struct RawDivision {
    id: String,
    label: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct RawAgent {
    id: String,
    name: String,
    division: String,
    role: String,
    #[serde(default)]
    capabilities: Vec<String>,
    system_prompt: String,
}

/// The validated roster.
#[derive(Debug, Clone)]
pub struct Roster {
    divisions: Vec<Division>,
    agents: Vec<AgentSpec>,
}

impl Roster {
    /// The compiled-in curated roster. Unwraps because the embedded TOML is
    /// known-good and validated in this crate's tests; arbitrary TOML must
    /// go through [`Roster::from_toml`].
    pub fn bundled() -> Self {
        Self::from_toml(ROSTER_TOML)
            .expect("embedded roster.toml is known-good and covered by tests")
    }

    /// Parse and validate a roster from TOML text.
    ///
    /// Validation: division and agent ids are unique and non-empty, every
    /// agent's division exists, and every agent declares at least one
    /// capability and a non-empty name, role, and system prompt.
    pub fn from_toml(raw: &str) -> Result<Self> {
        let parsed: RawRoster = toml::from_str(raw)?;

        let mut divisions = Vec::new();
        for d in parsed.division {
            if d.id.trim().is_empty() {
                return Err(Error::Other("roster: division id must not be empty".into()));
            }
            if divisions.iter().any(|e: &Division| e.id == d.id) {
                return Err(Error::Other(format!(
                    "roster: duplicate division id `{}`",
                    d.id
                )));
            }
            if d.label.trim().is_empty() {
                return Err(Error::Other(format!(
                    "roster: division `{}` needs a label",
                    d.id
                )));
            }
            divisions.push(Division {
                id: d.id,
                label: d.label,
                description: d.description,
            });
        }

        let mut agents = Vec::new();
        for a in parsed.agent {
            if a.id.trim().is_empty() {
                return Err(Error::Other("roster: agent id must not be empty".into()));
            }
            if a.id != a.id.to_lowercase() || a.id.contains(char::is_whitespace) {
                return Err(Error::Other(format!(
                    "roster: agent id `{}` must be a single lowercase word",
                    a.id
                )));
            }
            if agents.iter().any(|e: &AgentSpec| e.id == a.id) {
                return Err(Error::Other(format!(
                    "roster: duplicate agent id `{}`",
                    a.id
                )));
            }
            if !divisions.iter().any(|d| d.id == a.division) {
                return Err(Error::Other(format!(
                    "roster: agent `{}` references unknown division `{}`",
                    a.id, a.division
                )));
            }
            if a.capabilities.is_empty() {
                return Err(Error::Other(format!(
                    "roster: agent `{}` must declare at least one capability",
                    a.id
                )));
            }
            for cap in &a.capabilities {
                if cap.trim().is_empty() {
                    return Err(Error::Other(format!(
                        "roster: agent `{}` has an empty capability",
                        a.id
                    )));
                }
            }
            if a.name.trim().is_empty() {
                return Err(Error::Other(format!(
                    "roster: agent `{}` needs a name",
                    a.id
                )));
            }
            if a.role.trim().is_empty() {
                return Err(Error::Other(format!(
                    "roster: agent `{}` needs a role",
                    a.id
                )));
            }
            if a.system_prompt.trim().is_empty() {
                return Err(Error::Other(format!(
                    "roster: agent `{}` needs a system prompt",
                    a.id
                )));
            }
            agents.push(AgentSpec {
                id: a.id,
                name: a.name,
                division: a.division,
                role: a.role,
                capabilities: a.capabilities,
                system_prompt: a.system_prompt,
            });
        }

        Ok(Self { divisions, agents })
    }

    /// The agents, in roster file order.
    pub fn agents(&self) -> &[AgentSpec] {
        &self.agents
    }

    /// The divisions, in roster file order.
    pub fn divisions(&self) -> &[Division] {
        &self.divisions
    }

    /// Find an agent by id.
    pub fn find(&self, id: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// All agents declaring `cap`, in roster file order.
    pub fn by_capability(&self, cap: &str) -> Vec<&AgentSpec> {
        self.agents
            .iter()
            .filter(|a| a.capabilities.iter().any(|c| c == cap))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_roster_shape_is_12_agents_5_divisions() {
        let roster = Roster::bundled();
        assert_eq!(roster.agents().len(), 12);
        assert_eq!(roster.divisions().len(), 5);
        assert_eq!(
            roster
                .divisions()
                .iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "engineering",
                "research",
                "operations",
                "communication",
                "meta"
            ]
        );

        // unique agent ids, single lowercase words
        let mut ids: Vec<&str> = roster.agents().iter().map(|a| a.id.as_str()).collect();
        ids.sort();
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 12, "agent ids must be unique");
        for id in &ids {
            assert!(
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                    && !id.contains(char::is_whitespace),
                "agent id `{id}` must be a single lowercase word"
            );
        }
        assert_eq!(
            ids,
            vec![
                "atlas", "cipher", "ember", "forge", "keel", "orbit", "quill", "relay", "sable",
                "scout", "verse", "wren"
            ]
        );

        // every agent's division exists and carries a non-empty prompt
        for agent in roster.agents() {
            assert!(
                roster.divisions().iter().any(|d| d.id == agent.division),
                "agent `{}` has unknown division `{}`",
                agent.id,
                agent.division
            );
            assert!(!agent.capabilities.is_empty());
            let sentences = agent.system_prompt.matches(". ").count() + 1;
            assert!(
                (2..=4).contains(&sentences),
                "agent `{}` prompt should be 2-3 sentences",
                agent.id
            );
            assert!(
                agent.system_prompt.starts_with("I am "),
                "prompts are first-person"
            );
        }
    }

    #[test]
    fn find_and_by_capability_work() {
        let roster = Roster::bundled();
        let sable = roster.find("sable").expect("sable exists");
        assert_eq!(sable.name, "Sable");
        assert_eq!(sable.division, "engineering");
        assert!(roster.find("nonexistent").is_none());

        let planners = roster.by_capability("planning");
        assert_eq!(planners.len(), 3, "keel, relay, orbit plan");
        assert!(planners
            .iter()
            .all(|a| a.capabilities.contains(&"planning".to_string())));

        let memory = roster.by_capability("memory");
        assert_eq!(memory.len(), 3, "atlas, relay, ember hold memory");
        assert!(roster.by_capability("telepathy").is_empty());

        // the exact curated capability sets
        assert_eq!(
            roster.find("sable").unwrap().capabilities,
            vec!["code", "files", "shell"]
        );
        assert_eq!(
            roster.find("quill").unwrap().capabilities,
            vec!["code", "review"]
        );
        assert_eq!(
            roster.find("forge").unwrap().capabilities,
            vec!["code", "files"]
        );
        assert_eq!(
            roster.find("wren").unwrap().capabilities,
            vec!["testing", "code"]
        );
        assert_eq!(
            roster.find("cipher").unwrap().capabilities,
            vec!["security", "code"]
        );
        assert_eq!(
            roster.find("scout").unwrap().capabilities,
            vec!["web", "research"]
        );
        assert_eq!(
            roster.find("atlas").unwrap().capabilities,
            vec!["research", "memory"]
        );
        assert_eq!(roster.find("keel").unwrap().capabilities, vec!["planning"]);
        assert_eq!(
            roster.find("relay").unwrap().capabilities,
            vec!["planning", "memory"]
        );
        assert_eq!(roster.find("verse").unwrap().capabilities, vec!["writing"]);
        assert_eq!(
            roster.find("orbit").unwrap().capabilities,
            vec!["planning", "delegation"]
        );
        assert_eq!(roster.find("ember").unwrap().capabilities, vec!["memory"]);
    }

    #[test]
    fn from_toml_rejects_invalid_rosters() {
        // duplicate agent id
        let dup = ROSTER_TOML.replace("id = \"wren\"", "id = \"sable\"");
        assert!(Roster::from_toml(&dup).is_err());

        // unknown division
        let bad_div = ROSTER_TOML.replace(
            "division = \"engineering\"\nrole = \"Systems engineer\"",
            "division = \"marketing\"\nrole = \"Systems engineer\"",
        );
        assert!(Roster::from_toml(&bad_div).is_err());

        // no capabilities
        let no_caps = ROSTER_TOML.replace("capabilities = [\"writing\"]", "capabilities = []");
        assert!(Roster::from_toml(&no_caps).is_err());

        // uppercase agent id
        let upper = ROSTER_TOML.replace("id = \"verse\"", "id = \"Verse\"");
        assert!(Roster::from_toml(&upper).is_err());

        // not TOML at all
        assert!(Roster::from_toml("this is not toml = = =").is_err());

        // minimal valid roster
        let minimal = r#"
[[division]]
id = "solo"
label = "Solo"
description = "One agent."

[[agent]]
id = "soloist"
name = "Soloist"
division = "solo"
role = "Everything"
capabilities = ["code"]
system_prompt = "I am Soloist. I do the work. It gets done."
"#;
        let roster = Roster::from_toml(minimal).unwrap();
        assert_eq!(roster.agents().len(), 1);
        assert_eq!(roster.divisions().len(), 1);
    }
}
