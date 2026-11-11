//! astrid-tools — tool registry, risk ladder, permission gate, builtins.
//!
//! The 14 builtins are real implementations behind one pipeline: unknown
//! check → started receipt → RiskGuard → PreToolUse hooks → permission gate →
//! approval → execute → terminal receipt (always paired).

pub mod builtin;
pub mod confinement;
pub mod gate;
pub mod plugin;
pub mod registry;

pub use astrid_core::Risk;
pub use builtin::{
    AgentDelegate, CapabilityReport, FsEdit, FsGrep, FsLs, FsRead, FsWrite, MemForgetImage,
    MemRecallTool, MemRememberTool, ShellRun, SkillsList, SkillsLoad, WebFetch,
};
pub use gate::{GateDecision, PermissionGate, RiskGuard};
pub use plugin::{PluginManifest, PluginTool};
pub use registry::{
    refusal_reason_of, sanitize_fragment, ApprovalSink, DelegateSink, Tool, ToolCtx,
    ToolDefinition, ToolOutput, ToolRegistry,
};

/// Test-only support: tempdirs, recording approval/delegate sinks and a
/// ready-made ToolCtx. Kept out of the public API.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::registry::{ApprovalSink, DelegateSink, ToolCtx};
    use astrid_core::Event;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    /// Approval sink that records every request and answers with `approved`.
    pub(crate) struct RecordingApprovalSink {
        pub approved: bool,
        pub requested: Mutex<Vec<(String, String)>>,
    }

    impl RecordingApprovalSink {
        pub(crate) fn new(approved: bool) -> Self {
            Self {
                approved,
                requested: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn calls(&self) -> Vec<(String, String)> {
            self.requested.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ApprovalSink for RecordingApprovalSink {
        async fn request(&self, tool: &str, details: &str) -> bool {
            self.requested
                .lock()
                .unwrap()
                .push((tool.to_string(), details.to_string()));
            self.approved
        }
    }

    /// Delegate sink stub: records calls, returns a canned summary.
    pub(crate) struct StubDelegateSink {
        pub calls: Mutex<Vec<(String, String)>>,
        pub result: String,
    }

    impl StubDelegateSink {
        pub(crate) fn new(result: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: result.to_string(),
            }
        }
    }

    #[async_trait]
    impl DelegateSink for StubDelegateSink {
        async fn delegate(&self, agent_id: &str, task: &str) -> astrid_core::Result<String> {
            self.calls
                .lock()
                .unwrap()
                .push((agent_id.to_string(), task.to_string()));
            Ok(self.result.clone())
        }
    }

    /// Unique tempdir root per call.
    pub(crate) fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "astrid-tools-{tag}-{}-{}",
            std::process::id(),
            astrid_core::ids::new_id("t").replace('_', "")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fully wired ToolCtx over a real MemoryKernel and SkillIndex, plus the
    /// event receiver and the tempdir root (also the confinement root).
    pub(crate) async fn make_ctx() -> (ToolCtx, mpsc::Receiver<Event>, PathBuf) {
        let root = temp_root("ctx");
        let memory =
            Arc::new(astrid_memory::MemoryKernel::open(&root).expect("memory kernel opens"));
        let skills = Arc::new(astrid_workforce::SkillIndex::load(None).expect("skill index loads"));
        let (tx, rx) = mpsc::channel(1024);
        let ctx = ToolCtx {
            session_id: "test_session".to_string(),
            turn_id: "test_turn".to_string(),
            data_dir: root.clone(),
            confinement_root: root.clone(),
            memory,
            skills,
            events: tx,
            approval: Arc::new(RecordingApprovalSink::new(true)),
            delegate: Arc::new(StubDelegateSink::new("delegated task completed")),
            provider_desc: "test provider (mock — model test-model)".to_string(),
            agent_count: 12,
        };
        (ctx, rx, root)
    }
}
