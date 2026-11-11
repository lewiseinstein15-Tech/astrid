//! Receipt/event taxonomy v1 (docs/PROTOCOLS.md §2). Frozen set of 14 events.

use serde::{Deserialize, Serialize};

/// Risk classes, ordered by how much damage a misuse can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// Read-only.
    Safe,
    /// Writes Astrid-owned data (memory, sessions).
    Medium,
    /// Code execution on the host.
    High,
    /// Network egress to third parties.
    External,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::External => "external",
        }
    }
}

/// Honest turn outcomes. `unverified` means an answer was produced but no
/// check could confirm it; it is never reported as a plain success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    /// Zero model calls (small talk / capability question).
    CompletedFastPath,
    AwaitingApproval,
    Cancelled,
    Failed(String),
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NarrationKind {
    Ack,
    Finding,
    Progress,
    Decision,
    Completion,
}

/// A tool-call receipt. Started receipts are emitted before execution;
/// the terminal event (completed/failed/refused) closes the pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub call_id: String,
    pub tool: String,
    pub risk: Risk,
    /// Redacted JSON arguments (see [`redact_args`]).
    pub args_json: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub status: ReceiptStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReceiptStatus {
    Started,
    Completed,
    Failed,
    Refused,
}

/// The 14 taxonomy events. Serialized as `{"type": "...", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    SessionStarted { session_id: String, config_fingerprint: String },
    SessionEnded { session_id: String },
    TurnStarted { turn_id: String, input_preview: String },
    MessageDelta { turn_id: String, text: String },
    ThinkingDelta { turn_id: String, text: String },
    NarrationLine { turn_id: String, kind: NarrationKind, text: String },
    ToolStarted { receipt: Receipt },
    ToolCompleted { call_id: String, tool: String, result_preview: String, duration_ms: u64 },
    ToolFailed { call_id: String, tool: String, error: String, duration_ms: u64 },
    ToolRefused { call_id: String, tool: String, reason: String },
    ApprovalRequested { approval_id: String, tool: String, details: String },
    ApprovalResolved { approval_id: String, approved: bool },
    HookFired { event: String, script: String, decision: String },
    TurnCompleted {
        turn_id: String,
        outcome: TurnOutcome,
        /// Provider that served the turn ("offline" when none configured).
        provider: String,
        /// Rungs skipped before the first serving provider.
        ladder_slid: usize,
        /// Zero model calls iff fast path.
        model_calls: usize,
    },
}

/// Argument redaction: truncate long strings, blank key-like fields.
pub fn redact_args(args: &serde_json::Value) -> serde_json::Value {
    const MAX: usize = 200;
    fn scrub(key: &str, v: &serde_json::Value) -> serde_json::Value {
        let key_like = key.to_lowercase().contains("key")
            || key.to_lowercase().contains("token")
            || key.to_lowercase().contains("secret")
            || key.to_lowercase().contains("password")
            || key.to_lowercase().contains("authorization");
        match v {
            serde_json::Value::String(s) => {
                if key_like {
                    serde_json::Value::String("[REDACTED]".into())
                } else if s.len() > MAX {
                    serde_json::Value::String(format!("{}…[truncated {} chars]", &s[..MAX], s.len()))
                } else {
                    v.clone()
                }
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(|x| scrub(key, x)).collect())
            }
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, val) in map {
                    out.insert(k.clone(), scrub(k, val));
                }
                serde_json::Value::Object(out)
            }
            other => other.clone(),
        }
    }
    scrub("", args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redaction_blanks_keys_and_truncates() {
        let args = json!({
            "path": "/tmp/x",
            "api_key": "sk-123",
            "content": "x".repeat(300)
        });
        let out = redact_args(&args);
        assert_eq!(out["api_key"], "[REDACTED]");
        let content = out["content"].as_str().unwrap();
        assert!(content.ends_with("[truncated 300 chars]"));
        assert_eq!(out["path"], "/tmp/x");
    }

    #[test]
    fn event_roundtrips_with_type_tag() {
        let e = Event::TurnCompleted {
            turn_id: "turn_1".into(),
            outcome: TurnOutcome::CompletedFastPath,
            provider: "offline".into(),
            ladder_slid: 0,
            model_calls: 0,
        };
        let raw = serde_json::to_string(&e).unwrap();
        assert!(raw.contains("\"type\":\"turn_completed\""));
        let back: Event = serde_json::from_str(&raw).unwrap();
        match back {
            Event::TurnCompleted { provider, model_calls, .. } => {
                assert_eq!(provider, "offline");
                assert_eq!(model_calls, 0);
            }
            _ => panic!("wrong variant"),
        }
    }
}
