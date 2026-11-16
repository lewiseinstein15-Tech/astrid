//! Built-in keyless rungs (Phase 4.1). The 0.1.4 chip promised "kilo-auto/free"
//! but no rung existed behind it — every turn quietly fell to the offline
//! agent. These rungs are proven from this sandbox with curl (see
//! download/test-evidence/network-proof/): both answer a real chat request
//! with HTTP 200 and no API key.
//!
//! - kilo-auto/free: Kilo gateway, OpenAI-compatible, routes to the best
//!   free model (proven: nvidia/nemotron-3.5-super-vl:free, cost 0).
//! - pollinations: text.pollinations.ai/openai, anonymous tier
//!   (proven: user_tier "anonymous", gpt-oss-20b).
//!
//! A user's own provider (Settings > Providers) comes AFTER these two; the
//! offline agent stays last, always. Order is changeable in Settings.

use crate::openai::OpenAICompatProvider;

/// One built-in rung. `chat_path` is appended to `base_url` for the chat
/// endpoint (OpenAI gateways use /chat/completions; Pollinations uses /openai).
pub struct BuiltinRung {
    pub id: &'static str,
    pub base_url: &'static str,
    pub chat_path: &'static str,
    pub model: &'static str,
    pub desc: &'static str,
}

pub const KILO: BuiltinRung = BuiltinRung {
    id: "kilo-auto/free",
    base_url: "https://ai-gateway.kilo.ai/api/v1",
    chat_path: "/chat/completions",
    model: "kilo-auto/free",
    desc: "Kilo free gateway — automatic free-model routing (keyless)",
};

pub const POLLINATIONS: BuiltinRung = BuiltinRung {
    id: "pollinations",
    base_url: "https://text.pollinations.ai",
    chat_path: "/openai",
    model: "openai",
    desc: "Pollinations anonymous tier — gpt-oss-20b (keyless)",
};

/// Built-in ids in default order. The user's own rungs follow; offline is
/// implicit and last, as always.
pub const BUILTIN_IDS: [&str; 2] = [KILO.id, POLLINATIONS.id];

pub fn builtin_rung(id: &str) -> Option<&'static BuiltinRung> {
    // Field-access paths are not match patterns, so plain equality it is.
    if id == KILO.id {
        Some(&KILO)
    } else if id == POLLINATIONS.id {
        Some(&POLLINATIONS)
    } else {
        None
    }
}

/// A concrete provider for a built-in rung. Used by the ladder when the id
/// carries no user override.
pub fn builtin_provider(id: &str) -> Option<OpenAICompatProvider> {
    builtin_rung(id).map(|r| {
        OpenAICompatProvider::with_chat_path(r.id, r.base_url, r.model, None, r.chat_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Provider;

    #[test]
    fn builtin_ids_match_the_core_ladder_constant() {
        // The default ladder order lives in astrid-core (config default);
        // this mirror must never drift from it.
        assert_eq!(BUILTIN_IDS, astrid_core::BUILTIN_LADDER);
    }

    #[test]
    fn builtin_ids_match_constants() {
        assert_eq!(BUILTIN_IDS, ["kilo-auto/free", "pollinations"]);
    }

    #[test]
    fn builtin_providers_use_the_proven_endpoints() {
        let kilo = builtin_provider("kilo-auto/free").expect("kilo is built in");
        assert_eq!(kilo.id(), "kilo-auto/free");
        assert!(!kilo.is_local(), "kilo is a remote rung");

        let poll = builtin_provider("pollinations").expect("pollinations is built in");
        assert_eq!(poll.id(), "pollinations");
        assert!(!poll.is_local(), "pollinations is a remote rung");

        assert!(builtin_provider("offline").is_none(), "offline is not a built-in remote rung");
    }

    #[test]
    fn unknown_id_has_no_builtin() {
        assert!(builtin_rung("my-own-provider").is_none());
    }
}
