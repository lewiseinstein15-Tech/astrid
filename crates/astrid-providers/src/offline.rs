//! The offline agent — a first-class provider, not an error message. It is
//! deterministic, never fails, and never pretends to be a model: every receipt
//! trail it serves is honestly labeled `provider: offline`.

use crate::chat::{ChatRequest, ChatResponse, Provider, Role};
use astrid_core::Result;

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
        let content = if !req.tools.is_empty()
            && last_user.is_some_and(|m| is_capability_question(&m.content))
        {
            let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
            format!(
                "You are talking to Astrid's offline agent (no model provider configured). \
My live tools are: {}.\nI can execute approved tools and answer questions about my own \
capabilities from the registry. Configure a provider in .astrid/config.toml to unlock \
model-backed answers.",
                names.join(", ")
            )
        } else {
            FALLBACK.to_string()
        };
        Ok(ChatResponse {
            content: Some(content),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            provider: "offline".to_string(),
        })
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
    async fn offline_falls_back_for_non_capability_input() {
        // "list" starter but no capability noun.
        let out = OfflineProvider
            .chat(&req_with_tools(&["fs_read"], "list the files in src/"))
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
