//! Kernel intent gate: deterministic, zero-model-call answers for the inputs
//! that do not need an LLM. Ported in spirit from Jexi's `kernelGate` — the
//! lesson there was that a greeting must never cost a provider call.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Greetings, thanks, farewells — canned reply, zero model calls.
    SmallTalk,
    /// Questions about Astrid's own capabilities — answered from live state.
    CapabilityQuestion,
    /// Real work — goes to the agent turn loop.
    Task,
}

fn small_talk_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*(hi+|hello+|hey+|yo+|sup|good\s(morning|afternoon|evening)|thanks|thank\syou|thx|bye+|goodbye|good\snight|habari|poa|asante)[\s!.]*$")
            .unwrap()
    })
}

fn capability_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(what|which|how many|list|do you)\b.{0,40}\b(tools?|skills?|agents?|providers?|memory|hooks?|capabilities)\b")
            .unwrap()
    })
}

/// Classify user input. Order matters: small talk first, capability second.
pub fn classify(input: &str) -> Intent {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Intent::SmallTalk;
    }
    if small_talk_re().is_match(trimmed) {
        return Intent::SmallTalk;
    }
    if capability_re().is_match(trimmed) {
        return Intent::CapabilityQuestion;
    }
    Intent::Task
}

/// Deterministic small-talk reply pool.
pub fn small_talk_reply(input: &str) -> String {
    let lower = input.to_lowercase();
    if lower.contains("thank") || lower.contains("asante") {
        "Anytime. Say the word when there's real work.".to_string()
    } else if lower.contains("bye") || lower.contains("goodnight") || lower.contains("good night") {
        "Signing off. The terminal will keep your memory warm.".to_string()
    } else {
        format!(
            "Hello. I'm Astrid — ready for a task. Ask me what I can do, or hand me real work."
        )
    }
}

/// Capability answer built ONLY from live state handed in by the caller —
/// never from a hardcoded count that can drift from the registry.
pub fn capability_reply(
    tools: &[String],
    skills: &[String],
    agents: usize,
    provider_desc: &str,
    memory_pages: usize,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Right now: {} tools, {} bundled skills, {} staff agents, {} memory pages.\n",
        tools.len(),
        skills.len(),
        agents,
        memory_pages
    ));
    out.push_str(&format!("Provider: {provider_desc}.\n"));
    out.push_str("Tools: ");
    out.push_str(&tools.join(", "));
    if !skills.is_empty() {
        out.push_str("\nSkills: ");
        out.push_str(&skills.join(", "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_routes_correctly() {
        assert_eq!(classify("hello!"), Intent::SmallTalk);
        assert_eq!(classify("  THANKS "), Intent::SmallTalk);
        assert_eq!(classify("what tools do you have?"), Intent::CapabilityQuestion);
        assert_eq!(classify("list your skills"), Intent::CapabilityQuestion);
        assert_eq!(classify("write a fibonacci function in rust"), Intent::Task);
        assert_eq!(classify("how many memory pages do you keep"), Intent::CapabilityQuestion);
        // Task-looking input must never land in the lean lanes.
        assert_eq!(classify("fix the bug in main.rs then run tests"), Intent::Task);
        assert_eq!(classify("what is the capital of Kenya"), Intent::Task);
    }
}
