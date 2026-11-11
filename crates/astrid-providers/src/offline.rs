//! The offline agent — a first-class provider, not an error message. It is
//! deterministic, never fails, and never pretends to be a model: every receipt
//! trail it serves is honestly labeled `provider: offline`. It plans with
//! exact patterns only (no fuzzy intent guessing) and never emits a tool call
//! whose target tool is not in the offered menu.

use crate::chat::{ChatRequest, ChatResponse, Provider, Role, ToolCall};
use astrid_core::Result;
use regex::Regex;
use std::sync::OnceLock;

/// What the offline agent says when the question is not about Astrid itself.
const FALLBACK: &str = "You are talking to Astrid's offline agent (no model provider \
configured). I can answer questions about my own tools, skills, memory, and configuration, \
and execute approved tools. Configure a provider in .astrid/config.toml to unlock \
model-backed answers.";

const CAPABILITY_NOUNS: [&str; 5] = ["tool", "skill", "agent", "provider", "capability"];
const QUESTION_STARTERS: [&str; 5] = ["what", "which", "how many", "list", "do you"];

/// Capability-question phrasing: starts with an interrogative starter AND
/// mentions one of Astrid's own capability nouns (case-insensitive).
fn is_capability_question(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    QUESTION_STARTERS.iter().any(|s| lower.starts_with(s))
        && CAPABILITY_NOUNS.iter().any(|n| lower.contains(n))
}

fn write_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)^(?:create|write)\b.{0,60}?file (?:named |called )?([\w./-]+)\s+(?:containing|with (?:the )?content|containing the text)\s+["']?(.+?)["']?$"#).unwrap()
    })
}

fn write_reversed_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)^write\s+["'](.+?)["']\s+to\s+(?:the\s+)?file\s+([\w./-]+)\s*$"#).unwrap()
    })
}

fn read_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)^read (?:the )?file ([\w./-]+)\s*$"#).unwrap())
}

fn list_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)^list (?:the )?files?(?: (?:in|under) ([\w./-]+))?\s*$"#).unwrap())
}

fn remember_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)^remember (?:that )?(.{4,})\s*$"#).unwrap())
}

fn recall_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)^(?:recall|search|find)(?: my)?(?: memory| notes)? (?:for|about|with) (.+)\s*$"#).unwrap())
}

fn shell_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)^run (?:the )?(?:shell |command )+(.+?)\s*$"#).unwrap())
}

fn time_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*(?:what(?:'s| is)? )?(?:the )?(?:current )?time(?: is it| now)?\??\s*$"#)
            .unwrap()
    })
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:generate|create|make|give me|new)\b.{0,20}\b(?:uuid|guid)\b|\b(?:uuid|guid)\b.{0,20}\b(?:generate|create|please)\b"#)
            .unwrap()
    })
}

/// Generic extension-tool invocation: `call the tool <name> [with {json}]` —
/// works for MCP tools (`mcp_*`) and plugin tools (`plugin_*`) as well as any
/// builtin, but only when the tool is actually offered (the menu never grows).
fn call_tool_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)^call (?:the )?(?:mcp |plugin )?tool ([\w.:-]+)(?: with (.+))?\s*$"#)
            .unwrap()
    })
}

/// Web search: "search the web for <query>" (also "look up ... for ...").
fn search_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)^(?:search(?: the web)?(?: for)?|look(?: the web)?(?: up)? for)\s+(.+?)\??\s*$"#)
            .unwrap()
    })
}

/// Explicit URL fetch: "fetch https://… [and tell me what it is]".
fn fetch_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)^(?:fetch|open|get)\s+(?:the\s+)?(?:page\s+|url\s+)?(https?://[^\s]+?)(?:\s+and\s+.*)?\s*$"#)
            .unwrap()
    })
}

fn tool_matching<'a>(tools: &'a [crate::chat::ToolSpec], needle: &str) -> Option<&'a str> {
    tools
        .iter()
        .map(|t| t.name.as_str())
        .find(|n| n.contains(needle))
}

/// One deterministic plan step: (tool, args) — or None when no pattern
/// matches or the target tool is not in the offered menu.
fn plan(text: &str, tools: &[crate::chat::ToolSpec]) -> Option<ToolCall> {
    let offered = |name: &str| tools.iter().any(|t| t.name == name);
    let mk = |tool: &str, args: serde_json::Value| ToolCall {
        id: astrid_core::new_id("call"),
        name: tool.to_string(),
        arguments_json: args.to_string(),
    };
    let text = text.trim();

    if let Some(c) = shell_re().captures(text) {
        return offered("shell_run")
            .then(|| mk("shell_run", serde_json::json!({"command": c[1].trim()})));
    }
    if let Some(c) = write_re().captures(text) {
        let (path, content) = (&c[1], c[2].trim());
        return offered("fs_write").then(|| mk("fs_write", serde_json::json!({"path": path, "content": content})));
    }
    if let Some(c) = write_reversed_re().captures(text) {
        let (content, path) = (c[1].trim(), &c[2]);
        return offered("fs_write").then(|| mk("fs_write", serde_json::json!({"path": path, "content": content})));
    }
    if let Some(c) = read_re().captures(text) {
        return offered("fs_read").then(|| mk("fs_read", serde_json::json!({"path": &c[1]})));
    }
    if let Some(c) = list_re().captures(text) {
        let mut args = serde_json::Map::new();
        if let Some(p) = c.get(1) {
            args.insert("path".into(), serde_json::Value::String(p.as_str().to_string()));
        }
        return offered("fs_ls").then(|| mk("fs_ls", serde_json::Value::Object(args)));
    }
    if let Some(c) = remember_re().captures(text) {
        let body = c[1].trim();
        let topic: String = body.split_whitespace().take(5).collect::<Vec<_>>().join(" ");
        return offered("mem_remember").then(|| {
            mk(
                "mem_remember",
                serde_json::json!({"topic": topic, "body": body, "kind": "note"}),
            )
        });
    }
    if let Some(c) = recall_re().captures(text) {
        return offered("mem_recall")
            .then(|| mk("mem_recall", serde_json::json!({"query": c[1].trim(), "k": 5})));
    }
    // Extension tools (MCP / plugins): capability-shaped phrases route to the
    // first offered tool whose name carries the capability keyword.
    if time_re().is_match(text) {
        if let Some(name) = tool_matching(tools, "time") {
            return Some(mk(name, serde_json::json!({})));
        }
    }
    if uuid_re().is_match(text) {
        if let Some(name) = tool_matching(tools, "uuid") {
            return Some(mk(name, serde_json::json!({})));
        }
    }
    if let Some(c) = fetch_re().captures(text) {
        let url = c[1].trim().to_string();
        return offered("web_fetch")
            .then(|| mk("web_fetch", serde_json::json!({"url": url})));
    }
    if let Some(c) = search_re().captures(text) {
        let query = c[1].trim().to_string();
        return offered("web_search")
            .then(|| mk("web_search", serde_json::json!({"query": query, "max_results": 5})));
    }
    if let Some(c) = call_tool_re().captures(text) {
        let name = c[1].trim();
        let args_raw = c.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if offered(name) {
            let args: serde_json::Value = if args_raw.is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(args_raw) {
                    Ok(v) => v,
                    // Malformed JSON args are NOT a plan: fall through to the
                    // honest fallback rather than guessing.
                    Err(_) => return None,
                }
            };
            return Some(mk(name, args));
        }
    }
    None
}

pub struct OfflineProvider;

/// The tool call that produced the last tool observation: name + parsed args.
fn observed_call(req: &ChatRequest) -> (String, serde_json::Value, String) {
    let last_tool = req.messages.iter().rev().find(|m| m.role == Role::Tool);
    let call = last_tool.and_then(|t| {
        let id = t.tool_call_id.as_deref()?;
        req.messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant && m.tool_calls.iter().any(|c| c.id == id))
            .and_then(|m| m.tool_calls.iter().find(|c| c.id == id))
    });
    let (name, args_json) = call
        .map(|c| (c.name.clone(), c.arguments_json.clone()))
        .unwrap_or_else(|| ("unknown".to_string(), "{}".to_string()));
    let args: serde_json::Value = serde_json::from_str(&args_json).unwrap_or(serde_json::json!({}));
    let result = last_tool.map(|m| m.content.clone()).unwrap_or_default();
    (name, args, result)
}

/// Word-wrap a collapsed line of text into ≤width-char lines.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Compose the deterministic conclusion for a turn whose last step was a tool
/// observation. Every fact comes from the tool's own output — the composer
/// arranges, it never invents.
fn compose_turn_answer(req: &ChatRequest) -> String {
    let (tool, args, result) = observed_call(req);

    match tool.as_str() {
        "web_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("the query");
            // The tool formats clean markdown bullets; drop its own summary
            // header (first line + following blank) — the intro carries that.
            let mut body = result.lines().collect::<Vec<_>>();
            if body.first().is_some_and(|l| l.starts_with("Web search for")) {
                body = body
                    .into_iter()
                    .skip(1)
                    .skip_while(|l| l.trim().is_empty())
                    .collect();
            }
            format!(
                "Here's what I found on the web for **\"{query}\"** — live search, sources linked below.\n\n{}",
                body.join("\n")
            )
        }
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("the URL");
            let body = result.trim();
            let lines = wrap_words(body, 96);
            let mut gist: Vec<String> = lines.into_iter().take(6).collect();
            if body.chars().count() > 600 {
                gist.push("…(fetched text truncated — 8000-char cap on the tool)".to_string());
            }
            format!(
                "I fetched **{url}** and here's the gist:\n\n{}\n\nSource: {url}",
                gist.iter().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
            )
        }
        "shell_run" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let exit = result
                .lines()
                .find(|l| l.starts_with("exit "))
                .unwrap_or("exit ?")
                .trim()
                .to_string();
            let stdout: Vec<&str> = {
                let mut s = Vec::new();
                let mut in_stdout = false;
                for line in result.lines() {
                    if line == "--- stdout ---" {
                        in_stdout = true;
                    } else if line == "--- stderr ---" {
                        break;
                    } else if in_stdout && !line.trim().is_empty() {
                        s.push(line);
                    }
                }
                s
            };
            let block = if stdout.is_empty() {
                "(no stdout)".to_string()
            } else {
                stdout
                    .iter()
                    .take(5)
                    .map(|l| format!("  {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("Ran `{command}` — {exit}.\n\n{block}")
        }
        "fs_write" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("the file");
            let bytes: String = result
                .lines()
                .find(|l| l.contains("byte"))
                .unwrap_or("done")
                .trim()
                .to_string();
            format!("File saved — {bytes}.\n\nWrote the content to `{path}` as requested.")
        }
        "fs_read" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("the file");
            let body: Vec<&str> = result.lines().take(8).collect();
            format!(
                "Contents of `{path}`:\n\n```\n{}\n```",
                body.join("\n")
            )
        }
        "mem_remember" => {
            let body = args
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("your note");
            let page: String = result
                .split("page ")
                .nth(1)
                .map(|p| truncate_chars(p.trim_end_matches(')'), 40))
                .unwrap_or_default();
            if page.is_empty() {
                format!("Noted — I'll remember that **{body}**.")
            } else {
                format!("Noted — I'll remember that **{body}**.\n\nStored as memory page `{page}`.")
            }
        }
        "mem_recall" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("your query");
            let hits: Vec<&str> = result
                .lines()
                .filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit()))
                .collect();
            if hits.is_empty() {
                format!("No memory hits for **\"{query}\"** — nothing stored matches yet.")
            } else {
                format!(
                    "Found {} memory hit(s) for **\"{query}\"**:\n\n{}",
                    hits.len(),
                    hits.join("\n")
                )
            }
        }
        other => {
            let mut truncated: String = result.chars().take(400).collect();
            if result.chars().count() > 400 {
                truncated.push('…');
            }
            format!("`{other}` completed.\n\n{truncated}")
        }
    }
}

#[async_trait::async_trait]
impl Provider for OfflineProvider {
    fn id(&self) -> &str {
        "offline"
    }

    fn describe(&self) -> String {
        "offline deterministic agent (no provider configured)".to_string()
    }

    fn is_local(&self) -> bool {
        true
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let last_user = req.messages.iter().rev().find(|m| m.role == Role::User);
        let mut resp = ChatResponse {
            content: Some(FALLBACK.to_string()),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            provider: "offline".to_string(),
        };
        let Some(last_user) = last_user else {
            return Ok(resp);
        };
        let text = last_user.content.trim();

        // After a tool observation the planner concludes deterministically —
        // it never re-plans the same step (which would loop to the budget).
        // The conclusion is COMPOSED per tool: intro, structured body, source
        // line — never a raw dump of the receipt.
        if req
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Tool)
        {
            resp.content = Some(compose_turn_answer(req));
            return Ok(resp);
        }

        if !req.tools.is_empty() && is_capability_question(text) {
            let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
            resp.content = Some(format!(
                "You are talking to Astrid's offline agent (no model provider configured). \
My live tools are: {}.\nI can execute approved tools and answer questions about my own \
capabilities from the registry. Configure a provider in .astrid/config.toml to unlock \
model-backed answers.",
                names.join(", ")
            ));
            return Ok(resp);
        }

        if !req.tools.is_empty() {
            if let Some(call) = plan(text, &req.tools) {
                resp.tool_calls = vec![call];
                resp.content = None;
                resp.finish_reason = "tool_calls".to_string();
                return Ok(resp);
            }
        }

        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatMessage, ToolSpec};

    fn req_with_tools(tools: &[&str], last_user: &str) -> ChatRequest {
        ChatRequest {
            system: String::new(),
            messages: vec![
                ChatMessage::user("earlier prompt"),
                ChatMessage::user(last_user),
            ],
            tools: tools
                .iter()
                .map(|n| ToolSpec {
                    name: (*n).to_string(),
                    description: "test".to_string(),
                    parameters_json_schema: serde_json::json!({}),
                })
                .collect(),
            temperature: 0.0,
            max_tokens: None,
        }
    }

    #[tokio::test]
    async fn offline_answers_capability_questions_from_live_tools() {
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["fs_read", "shell_run"],
                "what tools do you have?",
            ))
            .await
            .unwrap();
        let content = out.content.unwrap();
        assert!(content.contains("fs_read"));
        assert!(content.contains("shell_run"));
        assert!(content.contains("offline"));
        assert_eq!(out.finish_reason, "stop");
        assert_eq!(out.provider, "offline");
        assert!(out.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn offline_plans_a_file_write() {
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["fs_write", "fs_read"],
                "create a file named hello.txt containing 'hello from astrid'",
            ))
            .await
            .unwrap();
        assert_eq!(out.finish_reason, "tool_calls");
        let call = &out.tool_calls[0];
        assert_eq!(call.name, "fs_write");
        let args: serde_json::Value = serde_json::from_str(&call.arguments_json).unwrap();
        assert_eq!(args["path"], "hello.txt");
        assert_eq!(args["content"], "hello from astrid");
    }

    #[tokio::test]
    async fn offline_plans_reversed_write_read_and_memory() {
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["fs_write"],
                "write \"line one\" to the file out/notes.md",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "fs_write");

        let out = OfflineProvider
            .chat(&req_with_tools(&["fs_read"], "read the file hello.txt"))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "fs_read");

        let out = OfflineProvider
            .chat(&req_with_tools(&["mem_remember"], "remember that the launch code is 7-3-1"))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "mem_remember");

        let out = OfflineProvider
            .chat(&req_with_tools(&["mem_recall"], "recall my memory for launch code"))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "mem_recall");
    }

    #[tokio::test]
    async fn offline_never_grows_the_menu() {
        // planner wants fs_write but it is not offered → honest fallback text
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["fs_read"],
                "create a file named hello.txt containing 'x'",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls.len(), 0);
        assert_eq!(out.finish_reason, "stop");
    }

    #[tokio::test]
    async fn offline_plans_a_web_search() {
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["web_search", "fs_read"],
                "search the web for rust programming language",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "web_search");
        let args: serde_json::Value =
            serde_json::from_str(&out.tool_calls[0].arguments_json).unwrap();
        assert_eq!(args["query"], "rust programming language");
        assert_eq!(args["max_results"], 5);
    }

    #[tokio::test]
    async fn offline_plans_a_url_fetch_without_the_trailing_clause() {
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["web_fetch"],
                "fetch https://example.com and tell me what it is",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "web_fetch");
        let args: serde_json::Value =
            serde_json::from_str(&out.tool_calls[0].arguments_json).unwrap();
        assert_eq!(args["url"], "https://example.com");
    }

    #[tokio::test]
    async fn composes_a_clean_search_answer_from_the_tool_output() {
        let result = "Web search for 'rust programming language' — 2 result(s) via wikipedia:\n\n\
- **Rust (programming language)** — Rust is a general-purpose language emphasizing performance and memory safety.\n  https://en.wikipedia.org/?curid=29414838\n\
- **Outline of the Rust programming language** — topical guide to Rust.\n  https://en.wikipedia.org/?curid=81297551\n";
        let req = ChatRequest {
            system: String::new(),
            tools: vec![ToolSpec {
                name: "web_search".into(),
                description: "t".into(),
                parameters_json_schema: serde_json::json!({}),
            }],
            messages: vec![
                ChatMessage::user("search the web for rust programming language"),
                ChatMessage {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: vec![crate::chat::ToolCall {
                        id: "call_1".into(),
                        name: "web_search".into(),
                        arguments_json: r#"{"query":"rust programming language"}"#.into(),
                    }],
                },
                ChatMessage::tool_result("call_1", result),
            ],
            temperature: 0.0,
            max_tokens: None,
        };
        let out = OfflineProvider.chat(&req).await.unwrap();
        let answer = out.content.unwrap();
        assert!(answer.contains("Here's what I found on the web"), "got: {answer}");
        assert!(answer.contains("\"rust programming language\""), "got: {answer}");
        assert!(answer.contains("**Rust (programming language)**"), "got: {answer}");
        assert!(answer.contains("https://en.wikipedia.org/?curid=29414838"), "got: {answer}");
        assert!(!answer.contains("Done. Tool result:"), "got: {answer}");
    }

    #[tokio::test]
    async fn composes_a_readable_shell_answer() {
        let result = "exit 0\n--- stdout ---\ndemo-proof-2026\n\n--- stderr ---\n\n";
        let req = ChatRequest {
            system: String::new(),
            tools: vec![ToolSpec {
                name: "shell_run".into(),
                description: "t".into(),
                parameters_json_schema: serde_json::json!({}),
            }],
            messages: vec![
                ChatMessage::user("run the shell command echo demo-proof-2026"),
                ChatMessage {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: vec![crate::chat::ToolCall {
                        id: "call_2".into(),
                        name: "shell_run".into(),
                        arguments_json: r#"{"command":"echo demo-proof-2026"}"#.into(),
                    }],
                },
                ChatMessage::tool_result("call_2", result),
            ],
            temperature: 0.0,
            max_tokens: None,
        };
        let out = OfflineProvider.chat(&req).await.unwrap();
        let answer = out.content.unwrap();
        assert!(answer.contains("Ran `echo demo-proof-2026`"), "got: {answer}");
        assert!(answer.contains("exit 0"), "got: {answer}");
        assert!(answer.contains("demo-proof-2026"), "got: {answer}");
        assert!(!answer.contains("--- stderr ---"), "got: {answer}");
    }

    #[tokio::test]
    async fn offline_plans_an_explicit_shell_command() {
        let out = OfflineProvider
            .chat(&req_with_tools(&["shell_run"], "run the shell command echo hi"))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "shell_run");
        let args: serde_json::Value =
            serde_json::from_str(&out.tool_calls[0].arguments_json).unwrap();
        assert_eq!(args["command"], "echo hi");
    }

    #[tokio::test]
    async fn offline_routes_capability_phrases_to_extension_tools() {
        // Time phrase → first offered tool whose name contains "time".
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["mcp_demo_time_now", "fs_read"],
                "what time is it?",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "mcp_demo_time_now");
        assert_eq!(out.tool_calls[0].arguments_json, "{}");

        // UUID phrase → offered uuid tool.
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["mcp_demo_new_uuid"],
                "generate a uuid please",
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "mcp_demo_new_uuid");

        // No matching tool offered → honest fallback, no invented call.
        let out = OfflineProvider
            .chat(&req_with_tools(&["fs_read"], "what time is it?"))
            .await
            .unwrap();
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.finish_reason, "stop");
    }

    #[tokio::test]
    async fn offline_generic_call_tool_pattern() {
        // Explicit MCP tool call with JSON args.
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["mcp_demo_echo_back"],
                r#"call the tool mcp_demo_echo_back with {"message":"hi there"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "mcp_demo_echo_back");
        let args: serde_json::Value =
            serde_json::from_str(&out.tool_calls[0].arguments_json).unwrap();
        assert_eq!(args["message"], "hi there");

        // Plugin tool, no args.
        let out = OfflineProvider
            .chat(&req_with_tools(&["plugin_wordcount"], "call the tool plugin_wordcount"))
            .await
            .unwrap();
        assert_eq!(out.tool_calls[0].name, "plugin_wordcount");
        assert_eq!(out.tool_calls[0].arguments_json, "{}");

        // Tool not offered → no call ever.
        let out = OfflineProvider
            .chat(&req_with_tools(&["fs_read"], "call the tool mcp_ghost_run"))
            .await
            .unwrap();
        assert!(out.tool_calls.is_empty());

        // Malformed JSON args → no call (honest fallback instead of guessing).
        let out = OfflineProvider
            .chat(&req_with_tools(
                &["mcp_demo_echo_back"],
                "call the tool mcp_demo_echo_back with {not json",
            ))
            .await
            .unwrap();
        assert!(out.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn offline_falls_back_for_unmatched_input() {
        let out = OfflineProvider
            .chat(&req_with_tools(&["fs_read"], "tell me a joke about compilers"))
            .await
            .unwrap();
        assert_eq!(out.content.unwrap(), FALLBACK);
    }

    #[tokio::test]
    async fn offline_falls_back_when_no_tools_are_offered() {
        let out = OfflineProvider
            .chat(&req_with_tools(&[], "what tools do you have?"))
            .await
            .unwrap();
        assert_eq!(out.content.unwrap(), FALLBACK);
    }

    #[test]
    fn offline_describes_itself_as_local() {
        let p = OfflineProvider;
        assert_eq!(p.id(), "offline");
        assert!(p.is_local());
        assert_eq!(
            p.describe(),
            "offline deterministic agent (no provider configured)"
        );
    }
}
