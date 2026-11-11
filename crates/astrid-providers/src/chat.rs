//! Provider-side chat vocabulary: roles, messages, tool specs and the
//! one-trait provider contract (`astrid-providers` §7 of ARCHITECTURE.md).

use serde_json::Value;

/// Who authored a [`ChatMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One tool invocation requested by the model.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON text the model produced for the call arguments.
    pub arguments_json: String,
}

/// A single message in the conversation transcript.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Set on [`Role::Tool`] messages: the [`ToolCall::id`] this result answers.
    pub tool_call_id: Option<String>,
    /// Set on assistant messages that request tool calls.
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(call_id.into()),
            tool_calls: Vec::new(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

/// A tool offered to the model, with its JSON-schema parameter object.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters_json_schema: Value,
}

/// A full chat round-trip request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}

impl Default for ChatRequest {
    fn default() -> Self {
        Self {
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            temperature: 0.7,
            max_tokens: None,
        }
    }
}

/// What a provider answered. `provider` is always the serving rung's id —
/// Astrid never lets a fallback masquerade as the configured model.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub provider: String,
}

/// The one provider contract every rung implements. Loopback endpoints are
/// "local"; the ladder uses that (not key presence) for its hermetic switch.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn describe(&self) -> String;
    fn is_local(&self) -> bool;
    async fn chat(&self, req: &ChatRequest) -> astrid_core::Result<ChatResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_helpers_set_roles() {
        let user = ChatMessage::user("hello");
        assert_eq!(user.role, Role::User);
        assert!(user.tool_call_id.is_none());

        let tool = ChatMessage::tool_result("call_1", "42");
        assert_eq!(tool.role, Role::Tool);
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));

        let sys = ChatMessage::system("be honest");
        assert_eq!(sys.role, Role::System);
    }
}
