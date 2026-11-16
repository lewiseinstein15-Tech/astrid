//! OpenAI-compatible chat-completions client over reqwest (rustls). Works
//! against any endpoint speaking the `/chat/completions` wire format, e.g.
//! Ollama (`http://127.0.0.1:11434/v1`), Groq, OpenRouter, vLLM.

use crate::chat::{ChatMessage, ChatRequest, ChatResponse, Provider, Role, StreamItem, ToolCall, ToolSpec};
use astrid_core::{Error, Result};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
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
    /// Path appended to base_url for the chat endpoint. Standard gateways use
    /// `/chat/completions`; Pollinations' anonymous endpoint lives at `/openai`.
    chat_path: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    pub fn new(id: &str, base_url: &str, model: &str, api_key: Option<String>) -> Self {
        Self::with_chat_path(id, base_url, model, api_key, "/chat/completions")
    }

    /// Full control over the chat path (built-in rungs need this for
    /// Pollinations, whose OpenAI-compatible endpoint is `POST /openai`).
    pub fn with_chat_path(
        id: &str,
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        chat_path: &str,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .build()
            .expect("reqwest client builds with valid configuration");
        Self {
            id: id.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            chat_path: chat_path.to_string(),
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
        let url = format!("{}{}", self.base_url, self.chat_path);
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

        // Reasoning-only guard (non-streaming twin of the pump guard):
        // reasoning models that run out of tokens before any content must
        // FAIL the rung so the ladder slides, instead of an empty answer.
        let finish = choice.finish_reason.clone().unwrap_or_default();
        if content.as_deref().map(str::trim).unwrap_or("").is_empty()
            && tool_calls.is_empty()
            && finish == "length"
        {
            return Err(Error::Provider(format!(
                "{}: model spent the whole token budget on reasoning and produced no content (finish_reason=length)",
                self.id
            )));
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason: if finish.is_empty() {
                "stop".to_string()
            } else {
                finish
            },
            provider: self.id.clone(),
        })
    }

    /// SSE streaming (`"stream": true`). Emits `Delta` items as content
    /// chunks arrive, `Thinking` for reasoning streams (e.g. DeepSeek's
    /// `reasoning_content`), and assembles tool-call fragments by index into
    /// the final `ChatResponse`.
    async fn chat_stream(
        &self,
        req: &ChatRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamItem>> {
        let url = format!("{}{}", self.base_url, self.chat_path);
        let mut body = build_request_body(req, &self.model);
        body["stream"] = Value::Bool(true);

        let (tx, rx) = tokio::sync::mpsc::channel::<StreamItem>(256);
        let mut call = self
            .client
            .post(&url)
            .header("accept", "text/event-stream")
            .json(&body);
        if let Some(key) = &self.api_key {
            call = call.bearer_auth(key);
        }

        // Pre-flight: connection/HTTP-status errors must slide the ladder
        // BEFORE any delta is committed, so build the response here and only
        // then hand the byte stream to the pump task.
        let response = match call.send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(StreamItem::Failed(format!(
                    "{}: request failed: {e}",
                    self.id
                ))).await;
                return Ok(rx);
            }
        };
        let status = response.status();
        if !status.is_success() {
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "unreadable error body".to_string());
            let _ = tx
                .send(StreamItem::Failed(format!(
                    "{}: HTTP {} {}",
                    self.id,
                    status.as_u16(),
                    Self::truncate_chars(text.trim(), ERROR_BODY_MAX_CHARS)
                )))
                .await;
            return Ok(rx);
        }

        let id = self.id.clone();
        let bytes = response.bytes_stream();
        tokio::spawn(async move {
            let mut stream = bytes;
            let mut content = String::new();
            let mut thinking = String::new();
            let mut finish_reason = String::new();
            // tool-call fragments keyed by stream index
            let mut calls: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
            let mut buf = String::new();
            let mut done = false;

            'outer: while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // SSE events are separated by blank lines; lines by \n.
                        while let Some(pos) = buf.find('\n') {
                            let line: String = buf.drain(..=pos).collect();
                            let line = line.trim_end_matches(['\n', '\r']);
                            if line.is_empty() || line.starts_with(':') {
                                continue;
                            }
                            let Some(data) = line.strip_prefix("data:") else {
                                continue;
                            };
                            let data = data.trim();
                            if data == "[DONE]" {
                                done = true;
                                break 'outer;
                            }
                            let Ok(v) = serde_json::from_str::<Value>(data) else {
                                continue;
                            };
                            // error objects mid-stream
                            if let Some(err) = v.get("error") {
                                let _ = tx
                                    .send(StreamItem::Failed(format!(
                                        "{id}: stream error: {err}"
                                    )))
                                    .await;
                                return;
                            }
                            let Some(choice) = v
                                .get("choices")
                                .and_then(Value::as_array)
                                .and_then(|c| c.first())
                            else {
                                continue;
                            };
                            if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                                finish_reason = fr.to_string();
                            }
                            let Some(delta) = choice.get("delta") else {
                                continue;
                            };
                            if let Some(reasoning) =
                                delta.get("reasoning_content").and_then(Value::as_str)
                            {
                                thinking.push_str(reasoning);
                                let _ = tx.send(StreamItem::Thinking(reasoning.to_string())).await;
                            }
                            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    content.push_str(text);
                                    let _ = tx.send(StreamItem::Delta(text.to_string())).await;
                                }
                            }
                            if let Some(tc_list) =
                                delta.get("tool_calls").and_then(Value::as_array)
                            {
                                for tc in tc_list {
                                    let idx =
                                        tc.get("index").and_then(Value::as_u64).unwrap_or(0)
                                            as usize;
                                    let entry = calls.entry(idx).or_default();
                                    if let Some(idv) = tc.get("id").and_then(Value::as_str) {
                                        entry.0.push_str(idv);
                                    }
                                    if let Some(fname) = tc
                                        .get("function")
                                        .and_then(|f| f.get("name"))
                                        .and_then(Value::as_str)
                                    {
                                        entry.1.push_str(fname);
                                    }
                                    if let Some(fargs) = tc
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(Value::as_str)
                                    {
                                        entry.2.push_str(fargs);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(StreamItem::Failed(format!("{id}: stream read failed: {e}")))
                            .await;
                        return;
                    }
                }
            }

            if !done && finish_reason.is_empty() && content.is_empty() && calls.is_empty() {
                let _ = tx
                    .send(StreamItem::Failed(format!(
                        "{id}: stream ended without data (connection closed early)"
                    )))
                    .await;
                return;
            }

            let tool_calls: Vec<ToolCall> = calls
                .into_values()
                .map(|(id, name, arguments)| ToolCall {
                    id,
                    name,
                    arguments_json: arguments,
                })
                .collect();

            // Reasoning-only guard: free-lane models (nemotron, gpt-oss) can
            // spend the whole token budget reasoning and finish with NO
            // content. That is a rung failure — slide, never an empty bubble.
            if content.is_empty() && tool_calls.is_empty() && finish_reason == "length" {
                let _ = tx
                    .send(StreamItem::Failed(format!(
                        "{id}: model spent the whole token budget on reasoning and produced no content (finish_reason=length)"
                    )))
                    .await;
                return;
            }

            let _ = tx
                .send(StreamItem::Final(ChatResponse {
                    content: if content.is_empty() { None } else { Some(content) },
                    tool_calls,
                    finish_reason: if finish_reason.is_empty() {
                        "stop".to_string()
                    } else {
                        finish_reason
                    },
                    provider: id,
                }))
                .await;
        });
        Ok(rx)
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

    #[tokio::test]
    async fn sse_stream_emits_deltas_then_final_with_assembled_tool_calls() {
        let events = vec![
            serde_json::json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"hel"}}]}).to_string(),
            serde_json::json!({"choices":[{"index":0,"delta":{"content":"lo "}}]}).to_string(),
            serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"name":"fs_ls","arguments":"{\"pa"}}]}}]}).to_string(),
            serde_json::json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\".\"}"}}]}}]}).to_string(),
            serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}).to_string(),
            "[DONE]".to_string(),
        ];
        let base = crate::test_support::spawn_sse_mock(events).await;
        let mut out = provider(&base)
            .chat_stream(&ChatRequest::default())
            .await
            .unwrap();
        let mut deltas = Vec::new();
        let mut final_resp = None;
        while let Some(item) = out.recv().await {
            match item {
                StreamItem::Delta(d) => deltas.push(d),
                StreamItem::Final(r) => final_resp = Some(r),
                other => panic!("unexpected item: {other:?}"),
            }
        }
        assert_eq!(deltas, vec!["hel".to_string(), "lo ".to_string()]);
        let resp = final_resp.expect("stream must end with Final");
        assert_eq!(resp.content.as_deref(), Some("hello "));
        assert_eq!(resp.finish_reason, "tool_calls");
        let call = &resp.tool_calls[0];
        assert_eq!(call.id, "call_9");
        assert_eq!(call.name, "fs_ls");
        assert_eq!(call.arguments_json, "{\"path\":\".\"}");
    }

    #[tokio::test]
    async fn sse_stream_error_yields_failed_item() {
        let base = crate::test_support::spawn_mock(crate::test_support::ERR_500_BODY).await;
        let mut out = provider(&base)
            .chat_stream(&ChatRequest::default())
            .await
            .unwrap();
        let mut items = Vec::new();
        while let Some(item) = out.recv().await {
            items.push(item);
        }
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], StreamItem::Failed(e) if e.contains("HTTP 500")));
    }
}
