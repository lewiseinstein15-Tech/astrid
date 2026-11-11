//! The 15 builtin tools, one group per file. All implementations are real —
//! no stubs — and every fs/memory path flows through the shared vocabulary
//! (confinement, memory verbs, skill index) rather than raw files.

mod fs;
mod memory;
mod meta;
mod shell;
mod skills;
mod web;

pub use fs::{FsEdit, FsGrep, FsLs, FsRead, FsWrite};
pub use memory::{MemForgetImage, MemRecallTool, MemRememberTool};
pub use meta::{AgentDelegate, CapabilityReport};
pub use shell::ShellRun;
pub use skills::{SkillsList, SkillsLoad};
pub use web::{WebFetch, WebSearch};

use astrid_core::{Error, Result};
use serde_json::Value;

/// Required string arg: missing and wrong-typed both refuse with a
/// "missing required arg" reason (the model may have sent `{"path": 3}`).
pub(crate) fn req_str(tool: &str, args: &Value, key: &str) -> Result<String> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(Error::ToolRefused {
            tool: tool.to_string(),
            reason: format!("missing required arg: {key} (wrong type, expected string)"),
        }),
        None => Err(Error::ToolRefused {
            tool: tool.to_string(),
            reason: format!("missing required arg: {key}"),
        }),
    }
}

/// Optional string arg; a wrong type is a refusal, not a silent default.
pub(crate) fn opt_str(tool: &str, args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Error::ToolRefused {
            tool: tool.to_string(),
            reason: format!("missing required arg: {key} (wrong type, expected string)"),
        }),
    }
}

/// Optional unsigned integer arg (usize).
pub(crate) fn opt_usize(tool: &str, args: &Value, key: &str) -> Result<Option<usize>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(v) => Ok(Some(v as usize)),
            None => Err(Error::ToolRefused {
                tool: tool.to_string(),
                reason: format!(
                    "missing required arg: {key} (wrong type, expected unsigned integer)"
                ),
            }),
        },
        Some(_) => Err(Error::ToolRefused {
            tool: tool.to_string(),
            reason: format!("missing required arg: {key} (wrong type, expected unsigned integer)"),
        }),
    }
}

/// Optional boolean arg.
pub(crate) fn opt_bool(tool: &str, args: &Value, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(Error::ToolRefused {
            tool: tool.to_string(),
            reason: format!("missing required arg: {key} (wrong type, expected boolean)"),
        }),
    }
}
