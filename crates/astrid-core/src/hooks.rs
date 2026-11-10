//! Hook contract v1 (docs/PROTOCOLS.md §3): exactly the 10 events Astrid
//! fires. External scripts get JSON on stdin and answer by exit code:
//! 0 = allow, 2 = block (stdout = reason), other = warn. Spawn errors fail
//! open — a broken hook must not brick the runtime — and the fail-open is
//! itself reported via a `hook.fired` receipt.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// The frozen hook event catalog: 10 events, all wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PermissionRequest,
    PermissionDenied,
    PostToolUse,
    PostToolUseFailure,
    TurnCompleted,
    PreCompact,
}

pub const HOOK_EVENT_COUNT: usize = 10;

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session_start",
            HookEvent::SessionEnd => "session_end",
            HookEvent::UserPromptSubmit => "user_prompt_submit",
            HookEvent::PreToolUse => "pre_tool_use",
            HookEvent::PermissionRequest => "permission_request",
            HookEvent::PermissionDenied => "permission_denied",
            HookEvent::PostToolUse => "post_tool_use",
            HookEvent::PostToolUseFailure => "post_tool_use_failure",
            HookEvent::TurnCompleted => "turn_completed",
            HookEvent::PreCompact => "pre_compact",
        }
    }

    pub fn all() -> [HookEvent; HOOK_EVENT_COUNT] {
        [
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::UserPromptSubmit,
            HookEvent::PreToolUse,
            HookEvent::PermissionRequest,
            HookEvent::PermissionDenied,
            HookEvent::PostToolUse,
            HookEvent::PostToolUseFailure,
            HookEvent::TurnCompleted,
            HookEvent::PreCompact,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    pub event: HookEvent,
    /// Command line to spawn (shell-interpreted).
    pub command: String,
    /// Glob-prefix matcher on tool name (`fs_*`, `shell_run`, `*`). None = all.
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookFile {
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
}

/// Prefix-glob matcher: `*` matches all, `fs_*` matches prefix.
pub fn matches(matcher: &str, tool: &str) -> bool {
    if matcher == "*" || matcher.is_empty() {
        return true;
    }
    if let Some(prefix) = matcher.strip_suffix('*') {
        return tool.starts_with(prefix);
    }
    matcher == tool
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "decision", rename_all = "lowercase")]
pub enum HookDecision {
    Allow,
    /// Exit code 2 — the call is blocked with this reason.
    Block { reason: String },
    /// Other non-zero exit — logged, never blocks.
    Warn { reason: String },
    /// Spawn error / timeout — the runtime continues, honestly labeled.
    FailOpen { reason: String },
}

/// Loaded hook registry.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    pub specs: Vec<HookSpec>,
}

impl HookRegistry {
    /// Load `<data_dir>/hooks.toml`. Missing file = empty registry (not an error).
    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("hooks.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let file: HookFile = toml::from_str(&raw)?;
        Ok(Self { specs: file.hooks })
    }

    /// Specs registered for this event whose matcher accepts the tool.
    pub fn specs_for(&self, event: &HookEvent, tool: &str) -> Vec<HookSpec> {
        self.specs
            .iter()
            .filter(|s| s.event == *event)
            .filter(|s| match &s.matcher {
                Some(m) => matches(m, tool),
                None => true,
            })
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }
}

/// Run one hook script: JSON payload on stdin, exit-code contract out.
pub async fn run_hook(spec: &HookSpec, payload: &serde_json::Value) -> HookDecision {
    let timeout_ms = spec.timeout_ms.unwrap_or(10_000);
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&spec.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("ASTRID_EVENT", spec.event.as_str())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return HookDecision::FailOpen {
                reason: format!("spawn failed: {e}"),
            }
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let body = serde_json::to_vec(payload).unwrap_or_default();
        let _ = stdin.write_all(&body).await;
        let _ = stdin.shutdown().await;
    }

    match timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            match out.status.code() {
                Some(0) => HookDecision::Allow,
                Some(2) => HookDecision::Block {
                    reason: if stdout.is_empty() { "hook blocked the call".into() } else { stdout },
                },
                Some(code) => HookDecision::Warn {
                    reason: format!("exit {code}: {stdout}"),
                },
                None => HookDecision::FailOpen {
                    reason: "terminated by signal".into(),
                },
            }
        }
        Ok(Err(e)) => HookDecision::FailOpen {
            reason: format!("wait failed: {e}"),
        },
        Err(_) => HookDecision::FailOpen {
            reason: format!("timeout after {timeout_ms}ms"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matcher_semantics() {
        assert!(matches("*", "anything"));
        assert!(matches("fs_*", "fs_write"));
        assert!(!matches("fs_*", "shell_run"));
        assert!(matches("shell_run", "shell_run"));
    }

    #[tokio::test]
    async fn exit_code_contract() {
        let allow = HookSpec {
            event: HookEvent::PreToolUse,
            command: "true".into(),
            matcher: None,
            timeout_ms: None,
        };
        let block = HookSpec {
            event: HookEvent::PreToolUse,
            command: "echo 'no writes' >&2; exit 2".into(),
            matcher: None,
            timeout_ms: None,
        };
        let warn = HookSpec {
            event: HookEvent::PreToolUse,
            command: "exit 7".into(),
            matcher: None,
            timeout_ms: None,
        };
        let payload = json!({"tool": "fs_write"});
        assert!(matches!(run_hook(&allow, &payload).await, HookDecision::Allow));
        assert!(matches!(run_hook(&block, &payload).await, HookDecision::Block { .. }));
        assert!(matches!(run_hook(&warn, &payload).await, HookDecision::Warn { .. }));
    }

    #[tokio::test]
    async fn spawn_failure_fails_open() {
        let spec = HookSpec {
            event: HookEvent::PreToolUse,
            command: "definitely-not-a-real-binary-xyz".into(),
            matcher: None,
            timeout_ms: None,
        };
        let payload = json!({"tool": "fs_write"});
        assert!(matches!(run_hook(&spec, &payload).await, HookDecision::FailOpen { .. }));
    }
}
