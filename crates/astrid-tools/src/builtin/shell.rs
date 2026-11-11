//! shell_run — code execution on the host, risk class High, with a hard
//! timeout, kill-on-timeout and bounded captured output.

use crate::registry::{truncate_chars, Tool, ToolCtx, ToolOutput};
use astrid_core::{Error, Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

use super::{opt_usize, req_str};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;
const OUTPUT_MAX_CHARS: usize = 8000;

pub struct ShellRun;

#[async_trait]
impl Tool for ShellRun {
    fn name(&self) -> &str {
        "shell_run"
    }
    fn description(&self) -> &str {
        "Run a shell command (`sh -c`) on the host; 30s default timeout (max 120s), output truncated to 8000 chars."
    }
    fn risk(&self) -> Risk {
        Risk::High
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command line"},
                "timeout_ms": {"type": "integer", "description": "Kill the process after this many ms (default 30000, max 120000)"}
            },
            "required": ["command"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let _ = ctx;
        let command = req_str(self.name(), args, "command")?;
        let timeout_ms = opt_usize(self.name(), args, "timeout_ms")?
            .map(|ms| ms as u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .max(100)
            .min(MAX_TIMEOUT_MS);

        let child = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let started = Instant::now();
        // The wait future OWNS the child: when the timeout drops it, the
        // kill_on_drop flag kills the process. The `Err(_)` branch below
        // therefore means "killed", with no handle left to clean up.
        let outcome =
            tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output()).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(Ok(output)) => {
                let exit_code = output.status.code();
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut content = match exit_code {
                    Some(code) => {
                        format!("exit {code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}")
                    }
                    None => format!(
                        "terminated by signal\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                    ),
                };
                let truncated = content.chars().count() > OUTPUT_MAX_CHARS;
                content = truncate_chars(&content, OUTPUT_MAX_CHARS);
                Ok(ToolOutput {
                    content,
                    meta_json: Some(json!({
                        "exit_code": exit_code,
                        "duration_ms": duration_ms,
                        "timed_out": false,
                        "truncated": truncated,
                    })),
                })
            }
            Ok(Err(e)) => Err(Error::Other(format!("shell_run spawn/wait failed: {e}"))),
            Err(_) => {
                // Timeout: the dropped wait future already killed the child.
                tracing::warn!(timeout_ms, "shell_run timed out and the process was killed");
                Ok(ToolOutput {
                    content: format!("timed out after {timeout_ms}ms (process killed)"),
                    meta_json: Some(json!({
                        "exit_code": Value::Null,
                        "duration_ms": duration_ms,
                        "timed_out": true,
                    })),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_ctx;

    #[tokio::test]
    async fn captures_stdout_stderr_and_exit_code() {
        let (ctx, _rx, _root) = make_ctx().await;
        let out = ShellRun
            .execute(&json!({"command": "echo shell-ok; echo oops >&2"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("shell-ok"), "got: {}", out.content);
        assert!(out.content.contains("oops"), "got: {}", out.content);
        assert!(out.content.contains("exit 0"));
        let meta = out.meta_json.unwrap();
        assert_eq!(meta["exit_code"], 0);
        assert_eq!(meta["timed_out"], false);
    }

    #[tokio::test]
    async fn timeout_kills_the_process() {
        let (ctx, _rx, _root) = make_ctx().await;
        let started = Instant::now();
        let out = ShellRun
            .execute(&json!({"command": "sleep 30", "timeout_ms": 300}), &ctx)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert!(
            out.content.contains("timed out after 300ms"),
            "got: {}",
            out.content
        );
        assert_eq!(out.meta_json.as_ref().unwrap()["timed_out"], true);
        assert!(out.meta_json.as_ref().unwrap()["exit_code"].is_null());
        assert!(
            elapsed < Duration::from_secs(5),
            "kill must be prompt, took {elapsed:?}"
        );
        std::fs::remove_dir_all(_root).ok();
    }

    #[tokio::test]
    async fn timeout_is_clamped_and_missing_args_refused() {
        let (ctx, _rx, _root) = make_ctx().await;
        // Missing command.
        let err = ShellRun.execute(&json!({}), &ctx).await.unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: command")
        );
        // Wrong type.
        let err = ShellRun
            .execute(&json!({"command": 3}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("wrong type"));
        // Timeout clamped into [100, 120000] — a huge value still executes fast.
        let out = ShellRun
            .execute(&json!({"command": "true", "timeout_ms": 999_999_999}), &ctx)
            .await
            .unwrap();
        assert_eq!(out.meta_json.unwrap()["timed_out"], false);
        std::fs::remove_dir_all(_root).ok();
    }
}
