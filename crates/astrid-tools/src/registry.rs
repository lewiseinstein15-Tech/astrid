//! The tool registry: the 15 builtins plus THE pipeline every call goes
//! through (ARCHITECTURE.md §3) — unknown check, started receipt, RiskGuard,
//! PreToolUse hooks, permission gate, approval, execute, terminal receipt.
//! Receipts always pair: exactly one terminal event per started receipt.

use astrid_core::ids::{new_id, now_iso};
use astrid_core::{
    redact_args, run_hook, Error, Event, HookDecision, HookEvent, HookRegistry, Profile, Receipt,
    ReceiptStatus, Result, Risk,
};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::gate::{GateDecision, PermissionGate, RiskGuard};

/// Who answers approval questions. `true` = approved.
#[async_trait::async_trait]
pub trait ApprovalSink: Send + Sync {
    async fn request(&self, tool: &str, details: &str) -> bool;
}

/// Who actually runs sub-agent turns. The tools crate routes; the binary
/// implements the real sub-turn.
#[async_trait::async_trait]
pub trait DelegateSink: Send + Sync {
    async fn delegate(&self, agent_id: &str, task: &str) -> Result<String>;
}

/// Everything a tool execution needs. Immutable from the tool's perspective;
/// the binary builds one per turn.
#[derive(Clone)]
pub struct ToolCtx {
    pub session_id: String,
    pub turn_id: String,
    pub data_dir: std::path::PathBuf,
    /// Every fs tool resolves paths inside this root.
    pub confinement_root: std::path::PathBuf,
    pub memory: Arc<astrid_memory::MemoryKernel>,
    pub skills: Arc<astrid_workforce::SkillIndex>,
    pub events: mpsc::Sender<Event>,
    pub approval: Arc<dyn ApprovalSink>,
    pub delegate: Arc<dyn DelegateSink>,
    pub provider_desc: String,
    pub agent_count: usize,
}

/// A single builtin or extension tool.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn risk(&self) -> Risk;
    /// JSON schema object: `{"type":"object","properties":{…},"required":[…]}`.
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput>;
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub meta_json: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_json_schema: Value,
}

pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    /// Profile knob for the pipeline's permission gate; the binary sets it
    /// from config (`registry.gate.profile = cfg.permissions`).
    pub gate: PermissionGate,
    /// Live mirror of registered tool names, shared with CapabilityReport.
    tool_names: Arc<RwLock<Vec<String>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Char-safe truncation (never splits a UTF-8 code point).
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…[truncated {} chars]", s.chars().count())
    }
}

/// Registry-safe tool-name fragment: lowercase ASCII alphanumerics, other
/// characters folded to `_`, capped at 40 chars. Shared by the plugin loader
/// and the MCP adapters so provenance prefixes stay consistent.
pub fn sanitize_fragment(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(40);
    out
}

async fn emit(ctx: &ToolCtx, event: Event) {
    // A dropped event bus must not fail the tool that emitted into it.
    let _ = ctx.events.send(event).await;
}

fn refusal_reason(e: &Error) -> String {
    match e {
        Error::ToolRefused { reason, .. } => reason.clone(),
        Error::HookBlocked(reason) => reason.clone(),
        other => other.to_string(),
    }
}

/// Human reason carried by refusal-shaped errors (`ToolRefused`,
/// `HookBlocked`); used by the binary to plumb refusals into tool results.
pub fn refusal_reason_of(e: &Error) -> String {
    refusal_reason(e)
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            gate: PermissionGate::new(Profile::default()),
            tool_names: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register the 15 builtins in menu order.
    pub fn with_builtins() -> Self {
        use crate::builtin::*;
        let mut registry = Self::new();
        for tool in [
            Arc::new(FsRead) as Arc<dyn Tool>,
            Arc::new(FsWrite),
            Arc::new(FsEdit),
            Arc::new(FsLs),
            Arc::new(FsGrep),
            Arc::new(ShellRun),
            Arc::new(MemRememberTool),
            Arc::new(MemRecallTool),
            Arc::new(MemForgetImage),
            Arc::new(SkillsList),
            Arc::new(SkillsLoad),
            Arc::new(WebFetch),
            Arc::new(WebSearch::new()),
            Arc::new(AgentDelegate),
            Arc::new(CapabilityReport::new(registry.tool_names.clone())),
        ] {
            registry.register(tool);
        }
        registry
    }

    /// Add a tool (builtins or bin-supplied extensions) and refresh the live
    /// name mirror CapabilityReport reads.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tool_names
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(tool.name().to_string());
        self.tools.push(tool);
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters_json_schema: t.parameters_schema(),
            })
            .collect()
    }

    /// THE pipeline (ARCHITECTURE.md §3). Every call, every time.
    pub async fn dispatch(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: Value,
        hooks: &HookRegistry,
        hook_timeout_ms: u64,
    ) -> Result<ToolOutput> {
        // 1. Unknown tools are refused before any receipt exists.
        let Some(tool) = self.get(name) else {
            return Err(Error::ToolRefused {
                tool: name.to_string(),
                reason: "unknown tool".to_string(),
            });
        };

        let call_id = new_id("call");
        let risk = tool.risk();
        let redacted = redact_args(&args);

        // 2. Started receipt (redacted args only — the raw args never leave).
        emit(
            ctx,
            Event::ToolStarted {
                receipt: Receipt {
                    call_id: call_id.clone(),
                    tool: name.to_string(),
                    risk,
                    args_json: serde_json::to_string(&redacted)
                        .unwrap_or_else(|_| "{}".to_string()),
                    started_at: now_iso(),
                    duration_ms: 0,
                    status: ReceiptStatus::Started,
                },
            },
        )
        .await;

        // 3. RiskGuard: refuse on the ACTUAL args (rm -rf, /etc writes, …).
        if let GateDecision::Deny(reason) = RiskGuard::refine(name, &args) {
            tracing::warn!(tool = name, %reason, "risk guard refused tool args");
            emit_refused(ctx, &call_id, name, &reason).await;
            return Err(Error::ToolRefused {
                tool: name.to_string(),
                reason,
            });
        }

        // 4. PreToolUse hooks, sequentially; Block refuses the call.
        let hook_payload = json!({
            "session_id": ctx.session_id,
            "turn_id": ctx.turn_id,
            "tool": name,
            "risk": risk.as_str(),
            "args": redacted,
        });
        for spec in hooks.specs_for(&HookEvent::PreToolUse, name) {
            let spec = with_timeout_floor(&spec, hook_timeout_ms);
            let decision = run_hook(&spec, &hook_payload).await;
            let (label, blocked) = match &decision {
                HookDecision::Allow => ("allow".to_string(), None),
                HookDecision::Block { reason } => ("block".to_string(), Some(reason.clone())),
                HookDecision::Warn { .. } => ("warn".to_string(), None),
                HookDecision::FailOpen { .. } => ("fail-open".to_string(), None),
            };
            emit(
                ctx,
                Event::HookFired {
                    event: HookEvent::PreToolUse.as_str().to_string(),
                    script: spec.command.clone(),
                    decision: label,
                },
            )
            .await;
            if let Some(reason) = blocked {
                tracing::warn!(tool = name, %reason, "pre_tool_use hook blocked the call");
                emit_refused(ctx, &call_id, name, &reason).await;
                return Err(Error::HookBlocked(reason));
            }
        }

        // 5. Permission gate: deny → refused; ask → approval sink.
        match self.gate.decision(risk) {
            GateDecision::Deny(reason) => {
                tracing::info!(tool = name, %reason, "permission gate denied tool");
                emit_refused(ctx, &call_id, name, &reason).await;
                return Err(Error::ToolRefused {
                    tool: name.to_string(),
                    reason,
                });
            }
            GateDecision::Ask => {
                let details = serde_json::to_string(&redacted).unwrap_or_default();
                let approval_id = new_id("approval");
                emit(
                    ctx,
                    Event::ApprovalRequested {
                        approval_id: approval_id.clone(),
                        tool: name.to_string(),
                        details: details.clone(),
                    },
                )
                .await;
                let approved = ctx.approval.request(name, &details).await;
                emit(
                    ctx,
                    Event::ApprovalResolved {
                        approval_id,
                        approved,
                    },
                )
                .await;
                if !approved {
                    tracing::info!(tool = name, "approval denied");
                    emit_refused(ctx, &call_id, name, "approval denied").await;
                    return Err(Error::ToolRefused {
                        tool: name.to_string(),
                        reason: "approval denied".to_string(),
                    });
                }
            }
            GateDecision::Allow => {}
        }

        // 6. Execute, then close the receipt pair honestly.
        let started = Instant::now();
        let result = tool.execute(&args, ctx).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(output) => {
                emit(
                    ctx,
                    Event::ToolCompleted {
                        call_id: call_id.clone(),
                        tool: name.to_string(),
                        result_preview: truncate_chars(&output.content, 500),
                        duration_ms,
                    },
                )
                .await;
                // PostToolUse hooks are non-blocking by contract: warn /
                // fail-open (even a Block) is logged, never acted on.
                let post_payload = json!({
                    "session_id": ctx.session_id,
                    "turn_id": ctx.turn_id,
                    "tool": name,
                    "result_preview": truncate_chars(&output.content, 500),
                });
                for spec in hooks.specs_for(&HookEvent::PostToolUse, name) {
                    let spec = with_timeout_floor(&spec, hook_timeout_ms);
                    let decision = run_hook(&spec, &post_payload).await;
                    emit(
                        ctx,
                        Event::HookFired {
                            event: HookEvent::PostToolUse.as_str().to_string(),
                            script: spec.command.clone(),
                            decision: decision_label(&decision),
                        },
                    )
                    .await;
                }
                Ok(output)
            }
            Err(e) => {
                let error = truncate_chars(&e.to_string(), 300);
                emit(
                    ctx,
                    Event::ToolFailed {
                        call_id: call_id.clone(),
                        tool: name.to_string(),
                        error: error.clone(),
                        duration_ms,
                    },
                )
                .await;
                let fail_payload = json!({
                    "session_id": ctx.session_id,
                    "turn_id": ctx.turn_id,
                    "tool": name,
                    "error": error,
                });
                for spec in hooks.specs_for(&HookEvent::PostToolUseFailure, name) {
                    let spec = with_timeout_floor(&spec, hook_timeout_ms);
                    let decision = run_hook(&spec, &fail_payload).await;
                    emit(
                        ctx,
                        Event::HookFired {
                            event: HookEvent::PostToolUseFailure.as_str().to_string(),
                            script: spec.command.clone(),
                            decision: decision_label(&decision),
                        },
                    )
                    .await;
                }
                Err(e)
            }
        }
    }
}

fn decision_label(decision: &HookDecision) -> String {
    match decision {
        HookDecision::Allow => "allow".to_string(),
        HookDecision::Block { .. } => "block".to_string(),
        HookDecision::Warn { .. } => "warn".to_string(),
        HookDecision::FailOpen { .. } => "fail-open".to_string(),
    }
}

/// The dispatch param is the config default; a per-hook `timeout_ms` overrides.
fn with_timeout_floor(spec: &astrid_core::HookSpec, default_ms: u64) -> astrid_core::HookSpec {
    let mut spec = spec.clone();
    if spec.timeout_ms.is_none() {
        spec.timeout_ms = Some(default_ms);
    }
    spec
}

async fn emit_refused(ctx: &ToolCtx, call_id: &str, tool: &str, reason: &str) {
    emit(
        ctx,
        Event::ToolRefused {
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            reason: reason.to_string(),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{make_ctx, temp_root, RecordingApprovalSink, StubDelegateSink};
    use serde_json::json;
    use std::collections::HashMap;

    async fn drain(rx: &mut mpsc::Receiver<Event>) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn count<'a>(events: &'a [Event], pred: impl Fn(&Event) -> bool + Copy) -> usize {
        events.iter().filter(|e| pred(e)).count()
    }

    /// call_id → number of terminal receipts (completed/failed/refused).
    fn terminal_pairs(events: &[Event]) -> HashMap<String, usize> {
        let mut started: HashMap<String, usize> = HashMap::new();
        let mut terminals: HashMap<String, usize> = HashMap::new();
        for event in events {
            match event {
                Event::ToolStarted { receipt } => {
                    started.insert(receipt.call_id.clone(), 1);
                }
                Event::ToolCompleted { call_id, .. }
                | Event::ToolFailed { call_id, .. }
                | Event::ToolRefused { call_id, .. } => {
                    *terminals.entry(call_id.clone()).or_default() += 1;
                }
                _ => {}
            }
        }
        let mut pairs = HashMap::new();
        for call_id in started.keys() {
            pairs.insert(
                call_id.clone(),
                terminals.get(call_id).copied().unwrap_or(0),
            );
        }
        pairs
    }

    #[tokio::test]
    async fn unknown_tool_refused_before_any_receipt() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let err = registry
            .dispatch(
                &ctx,
                "no_such_tool",
                json!({}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::ToolRefused { tool, reason } if tool == "no_such_tool" && reason == "unknown tool"),
            "got: {err}"
        );
        assert!(
            drain(&mut rx).await.is_empty(),
            "no receipt may exist for unknown tools"
        );
    }

    #[tokio::test]
    async fn successful_fs_write_pairs_started_with_completed() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let out = registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "notes/x.txt", "content": "hello"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        assert!(out.content.contains("wrote"));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolStarted { .. })),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolCompleted { .. })),
            1
        );
        // Redacted args travel on the started receipt.
        if let Some(Event::ToolStarted { receipt }) = events.first() {
            assert_eq!(receipt.tool, "fs_write");
            assert_eq!(receipt.risk, Risk::Medium);
            assert_eq!(receipt.status, ReceiptStatus::Started);
            assert!(receipt.args_json.contains("notes/x.txt"));
        } else {
            panic!("first event must be tool.started");
        }
    }

    #[tokio::test]
    async fn receipt_pairing_invariant_over_success_and_denial() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        // 1. Successful fs_write.
        registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "a.txt", "content": "x"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        // 2. Approval-denied shell_run (High risk → Ask path → denied).
        let denying = ToolCtx {
            approval: Arc::new(RecordingApprovalSink::new(false)),
            ..ctx.clone()
        };
        let err = registry
            .dispatch(
                &denying,
                "shell_run",
                json!({"command": "ls -la"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap_err();
        assert!(matches!(&err, Error::ToolRefused { reason, .. } if reason == "approval denied"));

        let events = drain(&mut rx).await;
        let pairs = terminal_pairs(&events);
        assert_eq!(pairs.len(), 2, "two started receipts expected");
        for (call_id, terminals) in &pairs {
            assert_eq!(
                *terminals, 1,
                "call {call_id} must have exactly one terminal receipt"
            );
        }
        // The Ask path emitted the approval pair.
        assert_eq!(
            count(&events, |e| matches!(e, Event::ApprovalRequested { .. })),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(
                e,
                Event::ApprovalResolved {
                    approved: false,
                    ..
                }
            )),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolRefused { .. })),
            1
        );
    }

    #[tokio::test]
    async fn approval_ask_then_approve_runs_the_tool() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let sink = Arc::new(RecordingApprovalSink::new(true));
        let ctx = ToolCtx {
            approval: sink.clone(),
            ..ctx
        };
        let registry = ToolRegistry::with_builtins();
        let out = registry
            .dispatch(
                &ctx,
                "shell_run",
                json!({"command": "echo ask-approved"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        assert!(out.content.contains("ask-approved"));
        // The sink saw the redacted-args details for shell_run.
        assert!(sink
            .calls()
            .iter()
            .any(|(tool, details)| tool == "shell_run" && details.contains("echo ask-approved")));
        let events = drain(&mut rx).await;
        // Ordered: started → approval.requested → approval.resolved → completed.
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::ToolStarted { .. } => "started",
                Event::ApprovalRequested { .. } => "approval_requested",
                Event::ApprovalResolved { .. } => "approval_resolved",
                Event::ToolCompleted { .. } => "completed",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "started",
                "approval_requested",
                "approval_resolved",
                "completed"
            ],
            "got: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn readonly_profile_denies_fs_write() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let mut registry = ToolRegistry::with_builtins();
        registry.gate.profile = Profile::ReadOnly;
        let err = registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "x", "content": "y"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap_err();
        assert!(matches!(&err, Error::ToolRefused { reason, .. } if reason.contains("readonly")));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolRefused { .. })),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolCompleted { .. })),
            0
        );
    }

    #[tokio::test]
    async fn risk_guard_deny_beats_profile() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let mut registry = ToolRegistry::with_builtins();
        registry.gate.profile = Profile::Full; // full would allow shell_run
        let err = registry
            .dispatch(
                &ctx,
                "shell_run",
                json!({"command": "rm -rf /"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("rm -rf"));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolRefused { .. })),
            1
        );
        // No approval row may appear — the call died at the RiskGuard stage.
        assert_eq!(
            count(&events, |e| matches!(e, Event::ApprovalRequested { .. })),
            0
        );
    }

    #[tokio::test]
    async fn pre_tool_use_hook_block_refuses_and_fires() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let hooks = HookRegistry {
            specs: vec![astrid_core::HookSpec {
                event: HookEvent::PreToolUse,
                // Exit-code contract: stdout carries the block reason.
                command: "echo 'writes are off'; exit 2".to_string(),
                matcher: None,
                timeout_ms: Some(5_000),
            }],
        };
        let err = registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "x", "content": "y"}),
                &hooks,
                5_000,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HookBlocked(reason) if reason.contains("writes are off")));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(
                &events,
                |e| matches!(e, Event::HookFired { decision, .. } if decision == "block")
            ),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolRefused { .. })),
            1
        );
    }

    #[tokio::test]
    async fn post_tool_use_hook_warns_without_blocking() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let hooks = HookRegistry {
            specs: vec![
                astrid_core::HookSpec {
                    event: HookEvent::PostToolUse,
                    command: "exit 7".to_string(), // warn, never blocks
                    matcher: None,
                    timeout_ms: Some(5_000),
                },
                astrid_core::HookSpec {
                    event: HookEvent::PostToolUseFailure,
                    command: "true".to_string(),
                    matcher: None,
                    timeout_ms: Some(5_000),
                },
            ],
        };
        let out = registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "x.txt", "content": "y"}),
                &hooks,
                5_000,
            )
            .await
            .unwrap();
        assert!(out.content.contains("wrote"));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(
                &events,
                |e| matches!(e, Event::HookFired { decision, .. } if decision == "warn")
            ),
            1
        );
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolCompleted { .. })),
            1
        );
    }

    #[tokio::test]
    async fn tool_failure_emits_failed_receipt_and_propagates() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let err = registry
            .dispatch(
                &ctx,
                "fs_read",
                json!({"path": "missing.txt"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Io(_)));
        let events = drain(&mut rx).await;
        assert_eq!(
            count(&events, |e| matches!(e, Event::ToolStarted { .. })),
            1
        );
        assert_eq!(count(&events, |e| matches!(e, Event::ToolFailed { .. })), 1);
    }

    #[tokio::test]
    async fn registry_exposes_names_definitions_and_redaction() {
        let registry = ToolRegistry::with_builtins();
        let names = registry.names();
        assert_eq!(names.len(), 15, "15 builtins registered: {names:?}");
        for expected in [
            "fs_read",
            "fs_write",
            "fs_edit",
            "fs_ls",
            "fs_grep",
            "shell_run",
            "mem_remember",
            "mem_recall",
            "mem_forget",
            "skills_list",
            "skills_load",
            "web_fetch",
            "agent_delegate",
            "capability_report",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing {expected}");
            let tool = registry.get(expected).expect("get returns the tool");
            assert_eq!(tool.name(), expected);
        }
        assert!(registry.get("definitely-not-a-tool").is_none());

        let defs = registry.definitions();
        assert_eq!(defs.len(), 15);
        for def in &defs {
            assert!(def.description.len() > 10, "{def:?}");
            assert_eq!(def.parameters_json_schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn risky_args_are_redacted_on_receipts() {
        let (ctx, mut rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        // auto profile + Medium tool → runs; api_key field must be redacted.
        registry
            .dispatch(
                &ctx,
                "fs_write",
                json!({"path": "a.txt", "content": "x", "api_key": "sk-super-secret"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        let events = drain(&mut rx).await;
        let started = events
            .iter()
            .find_map(|e| match e {
                Event::ToolStarted { receipt } => Some(receipt.clone()),
                _ => None,
            })
            .unwrap();
        assert!(
            started.args_json.contains("[REDACTED]"),
            "got: {}",
            started.args_json
        );
        assert!(!started.args_json.contains("sk-super-secret"));
        std::fs::remove_dir_all(_root).ok();
    }

    #[tokio::test]
    async fn delegate_and_report_flow_through_ctx() {
        let (ctx, _rx, _root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();

        let out = registry
            .dispatch(
                &ctx,
                "agent_delegate",
                json!({"agent_id": "scribe-01", "task": "summarize"}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "delegated task completed");

        let report = registry
            .dispatch(
                &ctx,
                "capability_report",
                json!({}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        assert!(
            report.content.contains("Tools (15)"),
            "got: {}",
            report.content
        );
        assert!(report.content.contains("test provider (mock"));
        assert!(report.content.contains("Staff agents: 12"));
        let _ = StubDelegateSink::new(""); // silence unused-import lint if any
        std::fs::remove_dir_all(_root).ok();
    }

    #[test]
    fn temp_roots_are_unique() {
        let a = temp_root("u1");
        let b = temp_root("u2");
        assert_ne!(a, b);
        std::fs::remove_dir_all(a).ok();
        std::fs::remove_dir_all(b).ok();
    }
}
