//! astrid-mcp — MCP (Model Context Protocol) client over the stdio transport.
//!
//! One [`McpClient`] per server process: JSON-RPC 2.0 on the child's
//! stdin/stdout, a handshake (`initialize` → `notifications/initialized`),
//! then `tools/list` discovery and `tools/call` execution. Every discovered
//! tool is surfaced as an [`McpToolAdapter`] implementing
//! `astrid_tools::Tool`, so MCP calls flow through the SAME receipt pipeline
//! (started receipt → RiskGuard → hooks → permission gate → approval →
//! execute → terminal receipt) as the builtins.
//!
//! Security posture (MCP spec: tool annotations are untrusted): tool risk
//! defaults to High — the auto permission profile asks for approval before
//! the first external call — unless the server config overrides the class.

use astrid_core::{Error, Result, Risk};
use astrid_tools::{Tool, ToolCtx, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

/// Protocol version Astrid negotiates. The 2024-11-05 revision is the
/// baseline every stdio server supports; servers may answer with a newer
/// string, which is accepted (the tool surface used here is unchanged).
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// One tool advertised by a server's `tools/list`.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A live connection to one stdio MCP server.
pub struct McpClient {
    server: String,
    child: Mutex<Child>,
    stdin_tx: mpsc::Sender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    next_id: AtomicU64,
    timeout: Duration,
    server_info: Value,
}

impl McpClient {
    /// Spawn the server process and complete the MCP handshake.
    pub async fn connect(
        cfg: &astrid_core::McpServerConfig,
        timeout: Duration,
    ) -> Result<Self> {
        let server = cfg.name.clone();
        if cfg.name.trim().is_empty() || cfg.command.trim().is_empty() {
            return Err(Error::Other(format!(
                "mcp server `{server}`: name and command are required"
            )));
        }

        let mut command = Command::new(&cfg.command);
        command
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // server logs must never corrupt the TUI
            .kill_on_drop(true);
        for (k, v) in &cfg.env {
            command.env(k, v);
        }
        let mut child = command
            .spawn()
            .map_err(|e| Error::Other(format!("mcp `{server}`: spawn {}: {e}", cfg.command)))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Other(format!("mcp `{server}`: no stdin")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Other(format!("mcp `{server}`: no stdout")))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);

        // writer: queue → child stdin (one JSON-RPC message per line)
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    return;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    return;
                }
                let _ = stdin.flush().await;
            }
            // channel closed: client dropped → child's kill_on_drop handles it
        });

        // reader: child stdout lines → resolve pending requests by id
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let Ok(v) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        let Some(id) = v.get("id").and_then(Value::as_u64) else {
                            continue; // notification: ignored in v1
                        };
                        let sender = reader_pending
                            .lock()
                            .ok()
                            .and_then(|mut p| p.remove(&id));
                        if let Some(tx) = sender {
                            let _ = tx.send(v);
                        }
                    }
                    _ => return, // EOF or error: child is gone
                }
            }
        });

        let mut client = Self {
            server: server.clone(),
            child: Mutex::new(child),
            stdin_tx,
            pending,
            next_id: AtomicU64::new(1),
            timeout,
            server_info: Value::Null,
        };

        // ---- handshake ----
        let result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "clientInfo": {"name": "astrid", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        client.server_info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        // serverInfo may be absent in lenient servers; protocolVersion check is
        // advisory only — a mismatched minor version still serves tools.
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    pub fn server_name(&self) -> &str {
        &self.server
    }

    pub fn server_info(&self) -> &Value {
        &self.server_info
    }

    /// Send a request and wait for the id-matched response. Returns the
    /// response object; a JSON-RPC error becomes `Err` with its message.
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = match self.pending.lock() {
                Ok(g) => g,
                Err(_) => {
                    return Err(Error::Other("mcp: pending map poisoned".to_string()));
                }
            };
            guard.insert(id, tx);
        }
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self
            .stdin_tx
            .send(line.to_string())
            .await
            .is_err()
        {
            return Err(Error::Other(format!(
                "mcp `{}`: server process is gone",
                self.server
            )));
        }

        let response = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(_)) => {
                return Err(Error::Other(format!(
                    "mcp `{}`: {method} dropped before answering",
                    self.server
                )))
            }
            Err(_) => {
                return Err(Error::Other(format!(
                    "mcp `{}`: {method} timed out after {}ms",
                    self.server,
                    self.timeout.as_millis()
                )))
            }
        };

        if let Some(err) = response.get("error") {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("protocol error");
            return Err(Error::Other(format!(
                "mcp `{}`: {method} failed: {msg}",
                self.server
            )));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// A notification: no id, no reply expected.
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let line = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.stdin_tx
            .send(line.to_string())
            .await
            .map_err(|_| Error::Other(format!("mcp `{}`: server process is gone", self.server)))
    }

    /// `tools/list` — the server's advertised tool surface.
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>> {
        let result = self.request("tools/list", json!({})).await?;
        let Some(tools) = result.get("tools").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        Ok(tools
            .iter()
            .filter_map(|t| {
                let name = t.get("name").and_then(Value::as_str)?.to_string();
                let description = t
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input_schema = t.get("inputSchema").cloned().unwrap_or(json!({"type":"object"}));
                Some(McpToolInfo {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect())
    }

    /// `tools/call` — text content joined; `isError: true` is an honest Err.
    pub async fn call_tool(&self, tool: &str, args: Value) -> Result<String> {
        let result = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await?;
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut text = String::new();
        if let Some(content) = result.get("content").and_then(Value::as_array) {
            for item in content {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = item.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
            }
        }
        if is_error {
            return Err(Error::Other(format!(
                "mcp `{}`: tool {tool} reported an error: {}",
                self.server,
                if text.is_empty() { "no detail" } else { &text }
            )));
        }
        Ok(text)
    }

    /// Structured result of `tools/call` when the server sent one.
    pub async fn call_tool_structured(&self, tool: &str, args: Value) -> Result<Value> {
        let result = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await?;
        Ok(result.get("structuredContent").cloned().unwrap_or(Value::Null))
    }

    pub async fn is_alive(&self) -> bool {
        match self.child.try_lock() {
            Ok(mut child) => child.try_wait().ok().flatten().is_none(),
            Err(_) => true, // a call in flight holds the lock; treat as alive
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // kill_on_drop(true) is set on the Command; dropping the Child kills
        // the server process.
        if let Ok(mut child) = self.child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

/// Sanitize `name` into a registry-safe tool fragment ([a-z0-9_]+).
/// Re-exported from astrid-tools so prefixes stay consistent everywhere.
pub fn sanitize_fragment(name: &str) -> String {
    astrid_tools::sanitize_fragment(name)
}

/// Parse a risk-class string from config; unknown values are refused.
pub fn parse_risk(s: &str) -> Result<Risk> {
    match s {
        "safe" => Ok(Risk::Safe),
        "medium" => Ok(Risk::Medium),
        "high" => Ok(Risk::High),
        "external" => Ok(Risk::External),
        other => Err(Error::Config(format!(
            "unknown risk class `{other}` (expected safe|medium|high|external)"
        ))),
    }
}

/// Heuristic risk for a server tool when the config sets no override:
/// mutating verbs in the name/description → High (ask), otherwise High
/// anyway — external code is untrusted. The knob exists so a trusted
/// internal server can be downgraded deliberately, not by default.
pub fn heuristic_risk(_info: &McpToolInfo) -> Risk {
    Risk::High
}

/// One MCP server tool, exposed through the standard Tool pipeline.
pub struct McpToolAdapter {
    client: Arc<McpClient>,
    info: McpToolInfo,
    tool_name: String,
    risk: Risk,
}

impl McpToolAdapter {
    pub fn new(client: Arc<McpClient>, info: McpToolInfo, risk: Risk) -> Self {
        let tool_name = format!("mcp_{}_{}", sanitize_fragment(client.server_name()), sanitize_fragment(&info.name));
        Self {
            client,
            info,
            tool_name,
            risk,
        }
    }

    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    pub fn info(&self) -> &McpToolInfo {
        &self.info
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.info.description
    }

    fn risk(&self) -> Risk {
        self.risk
    }

    fn parameters_schema(&self) -> Value {
        // MCP guarantees an object schema; enforce it defensively.
        if self.info.input_schema.get("type").and_then(Value::as_str) == Some("object") {
            self.info.input_schema.clone()
        } else {
            json!({"type": "object"})
        }
    }

    async fn execute(&self, args: &Value, _ctx: &ToolCtx) -> Result<ToolOutput> {
        let text = self.client.call_tool(&self.info.name, args.clone()).await?;
        Ok(ToolOutput {
            content: text,
            meta_json: Some(json!({
                "source": "mcp",
                "server": self.client.server_name(),
                "tool": self.info.name,
            })),
        })
    }
}

/// Connect one server and adapt its whole tool surface.
/// Returns the client plus its adapters (the client must stay alive for the
/// session duration — adapters hold an Arc to it).
pub async fn connect_and_adapt(
    cfg: &astrid_core::McpServerConfig,
    timeout_ms: u64,
) -> Result<(Arc<McpClient>, Vec<McpToolAdapter>)> {
    let client = Arc::new(McpClient::connect(cfg, Duration::from_millis(timeout_ms)).await?);
    let risk = match &cfg.default_risk {
        Some(r) => parse_risk(r)?,
        None => Risk::High,
    };
    let tools = client.list_tools().await?;
    let adapters = tools
        .into_iter()
        .map(|info| McpToolAdapter::new(client.clone(), info, risk))
        .collect();
    Ok((client, adapters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrid_core::McpServerConfig;

    fn python3_available() -> bool {
        std::process::Command::new("python3")
            .arg("-c")
            .arg("print(1)")
            .output()
            .is_ok()
    }

    /// Minimal MCP stdio server: initialize handshake + two tools.
    const DEMO_SERVER: &str = r#"
import json, sys, uuid
from datetime import datetime

TOOLS = [
    {"name": "echo_back", "description": "Echo the message back",
     "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]}},
    {"name": "new_uuid", "description": "Generate a fresh UUID",
     "inputSchema": {"type": "object", "additionalProperties": True}},
]

def main():
    for raw in sys.stdin:
        raw = raw.strip()
        if not raw:
            continue
        msg = json.loads(raw)
        method = msg.get("method", "")
        if "id" not in msg:
            continue
        if method == "initialize":
            result = {"protocolVersion": "2024-11-05",
                      "capabilities": {"tools": {}},
                      "serverInfo": {"name": "demo", "version": "0.1.0"}}
        elif method == "tools/list":
            result = {"tools": TOOLS}
        elif method == "tools/call":
            name = msg["params"]["name"]
            args = msg["params"].get("arguments", {})
            if name == "echo_back":
                text = "echo: " + str(args.get("message", ""))
                result = {"content": [{"type": "text", "text": text}], "isError": False}
            elif name == "new_uuid":
                result = {"content": [{"type": "text", "text": str(uuid.uuid4())}], "isError": False}
            elif name == "boom":
                result = {"content": [{"type": "text", "text": "exploded on demand"}], "isError": True}
            else:
                result = None
                send(msg, error={"code": -32602, "message": f"Unknown tool: {name}"})
                continue
        else:
            result = {}
        send(msg, result=result)

def send(msg, result=None, error=None):
    resp = {"jsonrpc": "2.0", "id": msg["id"]}
    if error is not None:
        resp["error"] = error
    else:
        resp["result"] = result
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()

main()
"#;

    async fn spawn_demo() -> (tempdir::TempDirGuard, McpClient) {
        let guard = tempdir::create("mcp-test");
        let script = guard.path.join("demo_server.py");
        std::fs::write(&script, DEMO_SERVER).unwrap();
        let cfg = McpServerConfig {
            name: "demo".to_string(),
            command: "python3".to_string(),
            args: vec![script.display().to_string()],
            ..Default::default()
        };
        let client = McpClient::connect(&cfg, Duration::from_secs(10))
            .await
            .expect("handshake completes");
        (guard, client)
    }

    mod tempdir {
        use std::path::PathBuf;
        pub struct TempDirGuard {
            pub path: PathBuf,
        }
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.path).ok();
            }
        }
        pub fn create(tag: &str) -> TempDirGuard {
            let path = std::env::temp_dir().join(format!(
                "astrid-mcp-{tag}-{}-{}",
                std::process::id(),
                astrid_core::ids::new_id("d").replace('_', "")
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDirGuard { path }
        }
    }

    #[tokio::test]
    async fn handshake_lists_and_calls() {
        if !python3_available() {
            eprintln!("python3 unavailable — skipping MCP integration test");
            return;
        }
        let (_guard, client) = spawn_demo().await;
        assert_eq!(client.server_name(), "demo");
        let info = client.server_info();
        assert_eq!(info["name"], "demo");

        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "echo_back");

        let out = client
            .call_tool("echo_back", serde_json::json!({"message": "mcp-wired"}))
            .await
            .unwrap();
        assert_eq!(out, "echo: mcp-wired");

        let uuid_text = client.call_tool("new_uuid", serde_json::json!({})).await.unwrap();
        assert_eq!(uuid_text.len(), 36, "a UUID string: {uuid_text}");

        let err = client
            .call_tool("boom", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("exploded"), "got: {err}");

        let err = client.call_tool("nope", serde_json::json!({})).await.unwrap_err();
        assert!(err.to_string().contains("Unknown tool"), "got: {err}");
    }

    #[tokio::test]
    async fn request_timeout_is_honest() {
        if !python3_available() {
            eprintln!("python3 unavailable — skipping MCP integration test");
            return;
        }
        let guard = tempdir::create("mcp-timeout");
        // A server that never answers any request.
        let script = guard.path.join("silent_server.py");
        std::fs::write(&script, "import sys\nfor raw in sys.stdin:\n  pass\n").unwrap();
        let cfg = McpServerConfig {
            name: "silent".to_string(),
            command: "python3".to_string(),
            args: vec![script.display().to_string()],
            ..Default::default()
        };
        let client = McpClient::connect(&cfg, Duration::from_millis(300)).await;
        // The handshake itself times out (initialize is a request).
        assert!(client.is_err(), "silent server must fail the handshake");
    }

    #[tokio::test]
    async fn adapter_names_and_risk_flow_through() {
        if !python3_available() {
            eprintln!("python3 unavailable — skipping MCP integration test");
            return;
        }
        let (_guard, client) = spawn_demo().await;
        let client = Arc::new(client);
        let tools = client.list_tools().await.unwrap();
        let adapter = McpToolAdapter::new(client.clone(), tools[0].clone(), Risk::High);
        assert_eq!(adapter.tool_name(), "mcp_demo_echo_back");
        assert_eq!(adapter.risk(), Risk::High);
        assert!(adapter.description().contains("Echo"));
        assert_eq!(adapter.parameters_schema()["type"], "object");
        assert_eq!(adapter.parameters_schema()["required"][0], "message");
    }

    #[test]
    fn sanitize_and_risk_parse() {
        assert_eq!(sanitize_fragment("Demo-Server.v2"), "demo_server_v2");
        assert_eq!(parse_risk("external").unwrap(), Risk::External);
        assert!(parse_risk("banana").is_err());
    }

    #[test]
    fn config_defaults_are_sane() {
        let cfg = McpServerConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.default_risk.is_none());
        assert!(cfg.env.is_empty());
    }
}
