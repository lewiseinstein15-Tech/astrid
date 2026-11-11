//! Manifest-based plugins: external tools declared in `plugins/*/plugin.toml`
//! under the data dir, executed as a fixed argv template with the tool
//! arguments appended as one JSON string. Plugins flow through the SAME
//! registry pipeline as builtins (receipts, RiskGuard, hooks, permission
//! gate, approval), so a plugin can never bypass the safety story.
//!
//! Manifest shape:
//! ```toml
//! name = "wordcount"
//! description = "Count words in a piece of text"
//! risk = "safe"                      # optional: safe|medium|high|external (default high)
//! run = ["bash", "{dir}/run.sh"]     # {dir} = the plugin's own directory
//! # optional JSON schema for arguments:
//! [args_schema]
//! type = "object"
//! properties.text = { type = "string" }
//! required = ["text"]
//! ```
//! Execution contract: the args JSON is appended to the argv as a single
//! final argument; 15s timeout; stdout/stderr captured; the plugin runs with
//! the session's working directory as cwd.

use crate::registry::{truncate_chars, Tool, ToolCtx, ToolOutput};
use astrid_core::{Error, Result, Risk};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;

const PLUGIN_TIMEOUT_MS: u64 = 15_000;
const OUTPUT_MAX_CHARS: usize = 4000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub name: String,
    pub description: String,
    /// safe|medium|high|external. Default high (a plugin is code execution).
    #[serde(default)]
    pub risk: Option<String>,
    /// Argv template; `{dir}` substitutes the plugin directory. The JSON args
    /// are appended as one extra argument at execution time.
    pub run: Vec<String>,
    /// Optional JSON schema (inline TOML table).
    #[serde(default)]
    pub args_schema: Value,
}

pub struct PluginTool {
    manifest: PluginManifest,
    dir: PathBuf,
    tool_name: String,
    risk: Risk,
}

impl PluginTool {
    pub fn from_manifest(dir: PathBuf, manifest: PluginManifest) -> Result<Self> {
        if manifest.name.trim().is_empty() {
            return Err(Error::Config(format!(
                "plugin at {}: `name` must not be empty",
                dir.display()
            )));
        }
        if manifest.run.is_empty() || manifest.run[0].trim().is_empty() {
            return Err(Error::Config(format!(
                "plugin `{}`: `run` must be a non-empty argv array",
                manifest.name
            )));
        }
        let tool_name = format!("plugin_{}", crate::registry::sanitize_fragment(&manifest.name));
        let risk = match manifest.risk.as_deref() {
            Some("safe") => Risk::Safe,
            Some("medium") => Risk::Medium,
            Some("high") => Risk::High,
            Some("external") => Risk::External,
            Some(other) => {
                return Err(Error::Config(format!(
                    "plugin `{}`: unknown risk `{other}` (safe|medium|high|external)",
                    manifest.name
                )))
            }
            None => Risk::High,
        };
        Ok(Self {
            manifest,
            dir,
            tool_name,
            risk,
        })
    }

    /// Scan `<data_dir>/plugins/*/plugin.toml` and load every valid manifest.
    /// Broken manifests are reported in the second vec — a bad plugin must be
    /// visible, never silent.
    pub fn scan(data_dir: &Path) -> (Vec<PluginTool>, Vec<String>) {
        let mut tools = Vec::new();
        let mut problems = Vec::new();
        let root = data_dir.join("plugins");
        let Ok(entries) = std::fs::read_dir(&root) else {
            return (tools, problems);
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let manifest_path = dir.join("plugin.toml");
            let manifest: PluginManifest = match std::fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|raw| toml::from_str(&raw).ok())
            {
                Some(m) => m,
                None => {
                    problems.push(format!(
                        "plugin {}: missing or invalid plugin.toml",
                        dir.display()
                    ));
                    continue;
                }
            };
            match Self::from_manifest(dir.clone(), manifest) {
                Ok(t) => {
                    let name = t.tool_name.clone();
                    if tools.iter().any(|t: &PluginTool| t.tool_name == name) {
                        problems.push(format!(
                            "plugin {}: duplicate tool name {name} (skipped)",
                            dir.display()
                        ));
                        continue;
                    }
                    tools.push(t);
                }
                Err(e) => problems.push(format!("plugin {}: {e}", dir.display())),
            }
        }
        (tools, problems)
    }

    pub fn manifest_name(&self) -> &str {
        &self.manifest.name
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.manifest.description
    }

    fn risk(&self) -> Risk {
        self.risk
    }

    fn parameters_schema(&self) -> Value {
        if self.manifest.args_schema.get("type").and_then(Value::as_str) == Some("object") {
            self.manifest.args_schema.clone()
        } else {
            json!({"type": "object"})
        }
    }

    async fn execute(&self, args: &Value, _ctx: &ToolCtx) -> Result<ToolOutput> {
        // Fixed argv: no shell interpolation of user content. The args JSON
        // rides as the last argv entry; {dir} is the only substitution and it
        // comes from disk, not from the model.
        let mut argv: Vec<String> = self
            .manifest
            .run
            .iter()
            .map(|part| part.replace("{dir}", &self.dir.display().to_string()))
            .collect();
        argv.push(serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string()));
        let (program, rest) = argv.split_first().expect("non-empty argv checked");

        let child = Command::new(program)
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();

        let child = match child {
            Ok(c) => c,
            Err(e) => {
                return Err(Error::Other(format!(
                    "plugin `{}`: spawn {program}: {e}",
                    self.manifest.name
                )))
            }
        };

        let started = Instant::now();
        let outcome = tokio::time::timeout(
            Duration::from_millis(PLUGIN_TIMEOUT_MS),
            child.wait_with_output(),
        )
        .await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(Ok(output)) => {
                let exit_code = output.status.code();
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let mut content = format!(
                    "exit {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                    exit_code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                    stdout,
                    stderr
                );
                let truncated = content.chars().count() > OUTPUT_MAX_CHARS;
                content = truncate_chars(&content, OUTPUT_MAX_CHARS);
                Ok(ToolOutput {
                    content,
                    meta_json: Some(json!({
                        "source": "plugin",
                        "plugin": self.manifest.name,
                        "exit_code": exit_code,
                        "duration_ms": duration_ms,
                        "truncated": truncated,
                    })),
                })
            }
            Ok(Err(e)) => Err(Error::Other(format!(
                "plugin `{}`: wait failed: {e}",
                self.manifest.name
            ))),
            Err(_) => Ok(ToolOutput {
                content: format!(
                    "timed out after {PLUGIN_TIMEOUT_MS}ms (plugin process killed)"
                ),
                meta_json: Some(json!({
                    "source": "plugin",
                    "plugin": self.manifest.name,
                    "exit_code": Value::Null,
                    "duration_ms": duration_ms,
                    "timed_out": true,
                })),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_plugin(dir: &Path, manifest: &str, script: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
        if let Some(s) = script {
            std::fs::write(dir.join("run.sh"), s).unwrap();
        }
    }

    #[test]
    fn scan_picks_up_valid_manifests_and_reports_broken_ones() {
        let root = std::env::temp_dir().join(format!(
            "astrid-plugins-scan-{}-{}",
            std::process::id(),
            astrid_core::ids::new_id("p").replace('_', "")
        ));
        std::fs::create_dir_all(&root).unwrap();

        write_plugin(
            &root.join("plugins").join("good"),
            r#"
name = "good"
description = "A well-formed plugin"
risk = "safe"
run = ["bash", "{dir}/run.sh"]
"#,
            Some("true"),
        );
        write_plugin(&root.join("plugins").join("bad_risk"), "name = 'x'\ndescription = 'y'\nrisk = 'banana'\nrun = ['true']", None);
        std::fs::create_dir_all(root.join("plugins").join("empty")).unwrap();

        let (tools, problems) = PluginTool::scan(&root);
        assert_eq!(tools.len(), 1, "only the good plugin loads");
        assert_eq!(tools[0].tool_name, "plugin_good");
        assert_eq!(tools[0].risk(), Risk::Safe);
        assert_eq!(problems.len(), 2, "broken manifests reported: {problems:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_risk_is_high_and_schema_defaults_to_object() {
        let root = std::env::temp_dir().join(format!(
            "astrid-plugins-risk-{}",
            astrid_core::ids::new_id("p").replace('_', "")
        ));
        write_plugin(
            &root.join("plugins").join("plain"),
            "name = 'plain'\ndescription = 'No risk given'\nrun = ['true']",
            None,
        );
        let (tools, problems) = PluginTool::scan(&root);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(tools[0].risk(), Risk::High);
        assert_eq!(tools[0].parameters_schema(), json!({"type": "object"}));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn execute_runs_argv_with_json_args_appended() {
        let root = std::env::temp_dir().join(format!(
            "astrid-plugins-exec-{}",
            astrid_core::ids::new_id("p").replace('_', "")
        ));
        write_plugin(
            &root.join("plugins").join("wordcount"),
            r#"
name = "wordcount"
description = "Count words in text"
risk = "safe"
run = ["bash", "{dir}/run.sh"]

[args_schema]
type = "object"
properties.text = { type = "string" }
"#,
            // args JSON arrives as $1 and MUST be forwarded explicitly.
            Some(r#"python3 -c 'import json,sys; d=json.loads(sys.argv[1]); print("words:", len(d.get("text","").split()))' "$1""#),
        );
        let (tools, problems) = PluginTool::scan(&root);
        assert!(problems.is_empty(), "{problems:?}");
        let tool = &tools[0];

        let (ctx, mut rx, _tmp) = crate::test_support::make_ctx().await;
        let out = tool
            .execute(&json!({"text": "the quick brown fox jumps over the lazy dog"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("words: 9"), "got: {}", out.content);
        assert_eq!(out.meta_json.as_ref().unwrap()["source"], "plugin");
        // A started receipt may not exist here (execute() bypasses dispatch),
        // but the event channel must still be untouched by direct execution.
        while rx.try_recv().is_ok() {}
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(_tmp).ok();
    }

    #[tokio::test]
    async fn execute_missing_script_is_an_honest_error() {
        let root = std::env::temp_dir().join(format!(
            "astrid-plugins-missing-{}",
            astrid_core::ids::new_id("p").replace('_', "")
        ));
        write_plugin(
            &root.join("plugins").join("ghost"),
            "name = 'ghost'\ndescription = 'Script does not exist'\nrun = ['bash', '{dir}/nope.sh']",
            None,
        );
        let (tools, problems) = PluginTool::scan(&root);
        assert!(problems.is_empty(), "{problems:?}");
        let (ctx, _rx, _tmp) = crate::test_support::make_ctx().await;
        // bash itself spawns fine; the missing script surfaces as an honest
        // non-zero exit with the reason on stderr — never a silent success.
        let out = tools[0]
            .execute(&json!({}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("exit 127"), "got: {}", out.content);
        assert!(out.content.contains("No such file or directory"), "got: {}", out.content);
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(_tmp).ok();
    }
}
