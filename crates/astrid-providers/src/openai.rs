//! OpenAI-compatible chat-completions client over reqwest (rustls). Works
//! against any endpoint speaking the `/chat/completions` wire format, e.g.
//! Ollama (`http://127.0.0.1:11434/v1`), Groq, OpenRouter, vLLM.

use crate::chat::{ChatMessage, ChatRequest, ChatResponse, Provider, Role, ToolCall, ToolSpec};
use astrid_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

/// Body-truncation cap for provider error strings.
const ERROR_BODY_MAX_CHARS: usize = 300;
/// Upper bound for one model round trip (generation can be slow; connect is
/// bounded separately so dead endpoints fail fast).
const REQUEST_TIMEOUT_SECS: u64 = 300;
const CONNECT_TIMEOUT_SECS: u64 = 10;

pub struct OpenAICompatProvider {
    id: String,
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    pub fn new(id: &str, base_url: &str, model: &str, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .build()
            .expect("reqwest client builds with valid configuration");
        Self {
            id: id.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key: api_key.filter(|k| !k.is_empty()),
            client,
        }
    }

    /// `true` when the endpoint host is loopback — the hermetic switch keys on
    /// this, never on key presence.
    pub fn endpoint_is_local(base_url: &str) -> bool {
        let rest = base_url
            .strip_prefix("http://")
            .or_else(|| base_url.strip_prefix("https://"))
            .unwrap_or(base_url);
        let authority = rest.split('/').next().unwrap_or("");
        let host = if authority.starts_with('[') {
            // Bracketed IPv6 literal, possibly `[::1]:11434`.
            authority
                .strip_prefix('[')
                .and_then(|s| s.split(']').next())
                .unwrap_or("")
        } else if authority.contains(':') && authority.matches(':').count() > 1 {
            // Bare IPv6 literal like `::1`.
            authority
        } else {
            authority.split(':').next().unwrap_or("")
        };
        matches!(
            host.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1"
        )
    }

    fn truncate_chars(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else {
            let head: String = s.chars().take(max).collect();
            format!("{head}…[truncated {} chars]", s.chars().count())
        }
    }
}

/// One message on the OpenAI wire. `content` is a plain string in this
/// client; tool results carry `tool_call_id`, assistant tool calls carry
/// `tool_calls`.
#[derive(Serialize)]
struct WireMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Serialize)]
struct WireToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    function: WireFunction,
}

#[derive(Serialize)]
struct WireFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: String,
    function: WireFunctionSpec,
}

#[derive(Serialize)]
struct WireFunctionSpec {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize)]
struct WireRequest {
    model: String,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

fn wire_message(m: &ChatMessage) -> WireMessage {
    let role = match m.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    WireMessage {
        role: role.to_string(),
        content: m.content.clone(),
        tool_call_id: m.tool_call_id.clone(),
        tool_calls: if m.tool_calls.is_empty() {
            None
        } else {
            Some(
                m.tool_calls
                    .iter()
                    .map(|c| WireToolCall {
                        id: c.id.clone(),
                        kind: "function".to_string(),
                        function: WireFunction {
                            name: c.name.clone(),
                            arguments: c.arguments_json.clone(),
                        },
                    })
                    .collect(),
            )
        },
    }
}

/// Build the exact JSON body the client posts. Split out as a pure function so
/// the wire shape is testable without HTTP.
fn build_request_body(req: &ChatRequest, model: &str) -> Value {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if !req.system.is_empty() {
        messages.push(WireMessage {
            role: "system".to_string(),
            content: req.system.clone(),
            tool_call_id: None,
            tool_calls: None,
        });
    }
    messages.extend(req.messages.iter().map(wire_message));

    let tools: Vec<WireTool> = req
        .tools
        .iter()
        .map(|t: &ToolSpec| WireTool {
            kind: "function".to_string(),
            function: WireFunctionSpec {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters_json_schema.clone(),
            },
        })
        .collect();

    let tool_choice = if tools.is_empty() {
        None
    } else {
        Some("auto".to_string())
    };

    let wire = WireRequest {
        model: model.to_string(),
        messages,
        tools,
        tool_choice,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
    };
    serde_json::to_value(&wire).expect("wire request always serializes")
}

/// Parsed pieces of a successful completion response (lenient: missing fields
/// default instead of failing the whole call).
#[derive(serde::Deserialize)]
struct WireResponse {
    choices: Vec<WireChoice>,
}

#[derive(serde::Deserialize)]
struct WireChoice {
    #[serde(default)]
    message: Option<WireResponseMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct WireResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireResponseToolCall>>,
}

#[derive(serde::Deserialize)]
struct WireResponseToolCall {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireResponseFunction>,
}

#[derive(serde::Deserialize)]
struct WireResponseFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[async_trait::async_trait]
impl Provider for OpenAICompatProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn describe(&self) -> String {
        let key = if self.api_key.is_some() {
            "key set"
        } else {
            "keyless"
        };
        format!(
            "{} ({} — model {}, {})",
            self.id, self.base_url, self.model, key
        )
    }

    fn is_local(&self) -> bool {
        Self::endpoint_is_local(&self.base_url)
    }

    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = build_request_body(req, &self.model);

        let mut call = self.client.post(&url).json(&body);
        // Keyless lanes MUST omit the Authorization header entirely — sending
        // `Bearer ` (empty) breaks several OpenAI-compatible servers.
        if let Some(key) = &self.api_key {
            call = call.bearer_auth(key);
        }
        let response = call
            .send()
            .await
            .map_err(|e| Error::Provider(format!("{}: request failed: {e}", self.id)))?;

        let status = response.status();
        // Honor Retry-After on rate limits / backpressure: surface the seconds
        // in the error text so the ladder can arm a matching cooldown.
        let retry_after_secs: Option<u64> = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());

        let text = response
            .text()
            .await
            .map_err(|e| Error::Provider(format!("{}: read body failed: {e}", self.id)))?;

        if !status.is_success() {
            let mut msg = format!(
                "{}: HTTP {} {}",
                self.id,
                status.as_u16(),
                Self::truncate_chars(text.trim(), ERROR_BODY_MAX_CHARS)
            );
            if let Some(secs) = retry_after_secs {
                msg.push_str(&format!(" (retry-after {secs}s)"));
            }
            return Err(Error::Provider(msg));
        }

        let parsed: WireResponse = serde_json::from_str(&text)
            .map_err(|e| Error::Provider(format!("{}: invalid completion JSON: {e}", self.id)))?;

        let choice = parsed
            .choices
            .first()
            .ok_or_else(|| Error::Provider(format!("{}: response had no choices", self.id)))?;

        let (content, tool_calls) = match &choice.message {
            Some(message) => {
                let calls = message
                    .tool_calls
                    .as_ref()
                    .map(|calls| {
                        calls
                            .iter()
                            .filter_map(|c| {
                                let function = c.function.as_ref()?;
                                Some(ToolCall {
                                    id: c.id.clone().unwrap_or_else(|| String::new()),
                                    name: function.name.clone().unwrap_or_default(),
                                    arguments_json: function.arguments.clone().unwrap_or_default(),
                                })
                            })
                            .collect::<Vec<ToolCall>>()
                    })
                    .unwrap_or_default();
                (message.content.clone(), calls)
            }
            None => (None, Vec::new()),
        };

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason: choice
                .finish_reason
                .clone()
                .unwrap_or_else(|| "stop".to_string()),
            provider: self.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatMessage;
    use serde_json::json;

    fn provider(base_url: &str) -> OpenAICompatProvider {
        OpenAICompatProvider::new("test-rung", base_url, "test-model", None)
    }

    #[test]
    fn body_shape_matches_openai_wire_format() {
        let req = ChatRequest {
            system: "Be honest.".into(),
            messages: vec![
                ChatMessage::user("list files"),
                ChatMessage::assistant(""),
                ChatMessage::tool_result("call_7", "main.rs"),
                ChatMessage::user("continue"),
            ],
            tools: vec![ToolSpec {
                name: "fs_ls".into(),
                description: "list a directory".into(),
                parameters_json_schema: json!({"type": "object"}),
            }],
            temperature: 0.2,
            max_tokens: Some(256),
        };
        let body = build_request_body(&req, "m1");
        assert_eq!(body["model"], "m1");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "Be honest.");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "call_7");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "fs_ls");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_tokens"], 256);
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.2).abs() < 1e-6, "temperature drifted: {temp}");
    }

    #[test]
    fn tool_choice_omitted_when_no_tools() {
        let req = ChatRequest::default();
        let body = build_request_body(&req, "m1");
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn locality_is_decided_by_endpoint_host() {
        assert!(OpenAICompatProvider::endpoint_is_local(
            "http://127.0.0.1:11434/v1"
        ));
        assert!(OpenAICompatProvider::endpoint_is_local(
            "http://localhost:8080"
        ));
        assert!(OpenAICompatProvider::endpoint_is_local(
            "http://[::1]:1234/v1"
        ));
        assert!(OpenAICompatProvider::endpoint_is_local("http://::1/v1"));
        assert!(!OpenAICompatProvider::endpoint_is_local(
            "https://example.com/v1"
        ));
        assert!(!OpenAICompatProvider::endpoint_is_local(
            "https://api.groq.com/openai/v1"
        ));
        assert!(provider("http://127.0.0.1:9/v1").is_local());
        assert!(!provider("https://example.com/v1").is_local());
    }

    #[tokio::test]
    async fn mock_200_yields_pong() {
        let base = crate::test_support::spawn_mock(crate::test_support::OK_BODY).await;
        let out = provider(&base).chat(&ChatRequest::default()).await.unwrap();
        assert_eq!(out.content.as_deref(), Some("pong"));
        assert_eq!(out.provider, "test-rung");
        assert_eq!(out.finish_reason, "stop");
        assert!(out.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn mock_500_reports_status_and_truncated_body() {
        let base = crate::test_support::spawn_mock(crate::test_support::ERR_500_BODY).await;
        let err = provider(&base)
            .chat(&ChatRequest::default())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("HTTP 500"), "got: {msg}");
        assert!(msg.contains("exploded"), "got: {msg}");
        assert!(matches!(err, Error::Provider(_)));
    }

    #[tokio::test]
    async fn retry_after_header_surfaces_in_error() {
        let base =
            crate::test_support::spawn_mock(crate::test_support::ERR_500_WITH_RETRY_AFTER).await;
        let err = provider(&base)
            .chat(&ChatRequest::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("(retry-after 7s)"), "got: {err}");
    }

    #[tokio::test]
    async fn keyless_lane_omits_authorization_header() {
        // The mock echoes captured request headers back in the JSON body.
        let base = crate::test_support::spawn_mock(crate::test_support::ECHO_HEADERS_BODY).await;
        let out = provider(&base).chat(&ChatRequest::default()).await.unwrap();
        let echoed = out.content.unwrap();
        assert!(
            !echoed.contains("authorization"),
            "keyless lane leaked a header: {echoed}"
        );

        let keyed = OpenAICompatProvider::new("k", &base, "m", Some("sk-test".into()));
        let out = keyed.chat(&ChatRequest::default()).await.unwrap();
        // Echo is lowercased by the mock.
        assert!(out
            .content
            .unwrap()
            .contains("authorization: bearer sk-test"));
    }

    #[test]
    fn describe_mentions_model_and_key_state() {
        let d = provider("http://127.0.0.1:11434/v1").describe();
        assert!(
            d.contains("test-model") && d.contains("keyless"),
            "got: {d}"
        );
    }
}
