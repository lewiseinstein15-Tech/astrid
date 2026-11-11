//! Meta tools: agent delegation routing and the deterministic capability
//! report. The binary implements the actual sub-turn behind DelegateSink;
//! here we only route and report live state handed in via ToolCtx.

use crate::registry::{Tool, ToolCtx, ToolOutput};
use astrid_core::{Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use super::req_str;

pub struct AgentDelegate;

#[async_trait]
impl Tool for AgentDelegate {
    fn name(&self) -> &str {
        "agent_delegate"
    }
    fn description(&self) -> &str {
        "Hand a task to a staff agent by id and get its result back (the runtime runs the sub-turn)."
    }
    fn risk(&self) -> Risk {
        Risk::Medium
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {"type": "string", "description": "Staff agent id from the roster"},
                "task": {"type": "string", "description": "Self-contained task description"}
            },
            "required": ["agent_id", "task"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let agent_id = req_str(self.name(), args, "agent_id")?;
        let task = req_str(self.name(), args, "task")?;
        let result = ctx.delegate.delegate(&agent_id, &task).await?;
        Ok(ToolOutput {
            content: result,
            meta_json: Some(json!({"agent_id": agent_id})),
        })
    }
}

/// Live capability report built ONLY from state handed in at runtime — no
/// hardcoded counts that can drift from the registry.
pub struct CapabilityReport {
    /// Shared, live mirror of registered tool names (owned by the registry).
    tool_names: Arc<RwLock<Vec<String>>>,
}

impl CapabilityReport {
    pub fn new(tool_names: Arc<RwLock<Vec<String>>>) -> Self {
        Self { tool_names }
    }
}

#[async_trait]
impl Tool for CapabilityReport {
    fn name(&self) -> &str {
        "capability_report"
    }
    fn description(&self) -> &str {
        "Deterministic live report: tool count and names, skills, memory pages, staff agents, provider."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let _ = args;
        let names = self
            .tool_names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let skills = ctx.skills.list();
        let content = format!(
            "Tools ({}): {}\nSkills: {} bundled\nMemory pages: {}\nStaff agents: {}\nProvider: {}",
            names.len(),
            names.join(", "),
            skills.len(),
            ctx.memory.page_count(),
            ctx.agent_count,
            ctx.provider_desc,
        );
        Ok(ToolOutput {
            content,
            meta_json: Some(json!({
                "tools": names,
                "skills": skills.len(),
                "memory_pages": ctx.memory.page_count(),
                "agents": ctx.agent_count,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ToolRegistry;
    use crate::test_support::make_ctx;
    use astrid_core::{Error, HookRegistry};

    #[tokio::test]
    async fn capability_report_is_deterministic_and_live() {
        let (ctx, _rx, root) = make_ctx().await;
        let registry = ToolRegistry::with_builtins();
        let first = registry
            .dispatch(
                &ctx,
                "capability_report",
                json!({}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        let again = registry
            .dispatch(
                &ctx,
                "capability_report",
                json!({}),
                &HookRegistry::default(),
                1_000,
            )
            .await
            .unwrap();
        assert_eq!(first.content, again.content, "deterministic report");
        assert!(
            first.content.contains("Tools (14)"),
            "got: {}",
            first.content
        );
        assert!(first.content.contains("fs_read"), "got: {}", first.content);
        assert!(first.content.contains("Provider: test provider (mock"));
        assert!(first.content.contains("Staff agents: 12"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn agent_delegate_routes_to_the_sink() {
        let (ctx, _rx, root) = make_ctx().await;
        let out = AgentDelegate
            .execute(
                &json!({"agent_id": "archivist-01", "task": "index the notes"}),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "delegated task completed");
        // Missing args refuse.
        let err = AgentDelegate
            .execute(&json!({"agent_id": "a"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: task")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
