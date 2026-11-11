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

/// One deterministic plan step: (tool, args) — or None when no pattern
/// matches or the target tool is not offered.
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
    None
}

pub struct OfflineProvider;

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
        if req
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Tool)
        {
            let result = req
                .messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Tool)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            let mut truncated: String = result.chars().take(400).collect();
            if result.chars().count() > 400 {
                truncated.push('…');
            }
            resp.content =
                Some(format!("Done. Tool result: {truncated}"));
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
