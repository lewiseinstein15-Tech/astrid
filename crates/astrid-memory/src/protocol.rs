//! astrid-memory protocol v1.0 — the frozen five-verb surface.
//!
//! Frozen contract (docs/PROTOCOLS.md §1): every memory mutation is a verb
//! call on the [`crate::MemoryKernel`]; tools and the agent never touch
//! memory files directly. A protocol change bumps [`PROTOCOL_VERSION`] and
//! updates every caller in the same commit — there is no compatibility shim.

/// Protocol version of the memory five-verb surface.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Envelope error codes (docs/PROTOCOLS.md §1).
pub const E_UNKNOWN_VERB: &str = "E_UNKNOWN_VERB";
pub const E_BAD_ARG: &str = "E_BAD_ARG";
pub const E_NOT_FOUND: &str = "E_NOT_FOUND";
pub const E_CONFINEMENT: &str = "E_CONFINEMENT";

/// The five protocol verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Remember,
    Recall,
    Entity,
    Synthesize,
    Forget,
}

impl Verb {
    /// Wire name of the verb, e.g. `"remember"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Verb::Remember => "remember",
            Verb::Recall => "recall",
            Verb::Entity => "entity",
            Verb::Synthesize => "synthesize",
            Verb::Forget => "forget",
        }
    }

    /// Parse a verb from its wire name.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "remember" => Some(Verb::Remember),
            "recall" => Some(Verb::Recall),
            "entity" => Some(Verb::Entity),
            "synthesize" => Some(Verb::Synthesize),
            "forget" => Some(Verb::Forget),
            _ => None,
        }
    }
}

/// One hit from `recall`: a page plus per-stage scores and the fused score.
///
/// `stages` carries the raw per-stage scores, e.g.
/// `[("bm25", 0.8), ("recency", 0.3), ("phrase", 1.0)]`; `score` is the
/// deterministic Reciprocal Rank Fusion over the three stage rankings.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
    pub stages: Vec<(String, f64)>,
}

/// Typed entity page returned by `entity`.
///
/// `edges` are `(relation, target_name)` pairs declared on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityPage {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub edges: Vec<(String, String)>,
}

/// Returned by `forget`: the tombstone written into the page frontmatter.
#[derive(Debug, Clone, PartialEq)]
pub struct Tombstone {
    pub id: String,
    pub reason: String,
    pub at: String,
}

/// Lightweight page listing entry.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSummary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub forgotten: bool,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_frozen_at_1_0() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
    }

    #[test]
    fn verbs_roundtrip_through_wire_names() {
        let all = [
            Verb::Remember,
            Verb::Recall,
            Verb::Entity,
            Verb::Synthesize,
            Verb::Forget,
        ];
        for verb in all {
            assert_eq!(Verb::parse(verb.as_str()), Some(verb));
        }
        assert_eq!(Verb::parse("remember "), Some(Verb::Remember));
        assert_eq!(Verb::parse("destroy"), None);
    }
}
