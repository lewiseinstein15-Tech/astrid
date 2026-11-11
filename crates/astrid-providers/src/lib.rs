//! astrid-providers — provider ladder, OpenAI-compatible client, offline fallback.

pub mod chat;
pub mod ladder;
pub mod offline;
pub mod openai;

pub use chat::{ChatMessage, ChatRequest, ChatResponse, Provider, Role, ToolCall, ToolSpec};
pub use ladder::{AttemptRecord, LadderResult, ProviderLadder};
pub use offline::OfflineProvider;
pub use openai::OpenAICompatProvider;

/// Test-only hermetic HTTP mock: a hand-rolled HTTP/1.1 server on a tokio
/// `TcpListener` bound to `127.0.0.1:0`. No new dependencies.
#[cfg(test)]
pub(crate) mod test_support {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    pub(crate) struct MockSpec {
        pub status_line: &'static str,
        pub body: &'static str,
        pub retry_after: Option<u64>,
        /// Echo the captured request headers back as the completion content
        /// (used to assert which headers the client actually sent).
        pub echo_headers: bool,
    }

    pub(crate) const OK_BODY: MockSpec = MockSpec {
        status_line: "200 OK",
        body: "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"pong\",\"tool_calls\":[]},\"finish_reason\":\"stop\"}]}",
        retry_after: None,
        echo_headers: false,
    };

    pub(crate) const ERR_500_BODY: MockSpec = MockSpec {
        status_line: "500 Internal Server Error",
        body: "server exploded",
        retry_after: None,
        echo_headers: false,
    };

    pub(crate) const ERR_500_WITH_RETRY_AFTER: MockSpec = MockSpec {
        status_line: "500 Internal Server Error",
        body: "rate limited",
        retry_after: Some(7),
        echo_headers: false,
    };

    pub(crate) const ECHO_HEADERS_BODY: MockSpec = MockSpec {
        status_line: "200 OK",
        body: "",
        retry_after: None,
        echo_headers: true,
    };

    /// Bind `127.0.0.1:0` and serve the spec for every connection until the
    /// test ends. Returns the OpenAI-compatible base_url (without the path).
    pub(crate) async fn spawn_mock(spec: MockSpec) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                serve_one(stream, &spec);
            }
        });
        format!("http://{addr}/v1")
    }

    fn serve_one(stream: TcpStream, spec: &MockSpec) {
        // Owned copies for the spawned task (spec fields are 'static str but
        // the reference itself is not).
        let status_line = spec.status_line.to_string();
        let static_body = spec.body.to_string();
        let retry_after = spec.retry_after;
        let echo_headers = spec.echo_headers;
        let mut stream = stream;
        tokio::spawn(async move {
            let mut buf: Vec<u8> = Vec::with_capacity(8192);
            let mut chunk = [0u8; 4096];
            let mut headers_end: Option<usize> = None;
            let mut content_length: usize = 0;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let have_full_body = match headers_end {
                    Some(he) => buf.len() >= he + 4 + content_length,
                    None => false,
                };
                if have_full_body {
                    break;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
                    Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                    Ok(Ok(n)) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if headers_end.is_none() {
                            if let Some(pos) = find_headers_end(&buf) {
                                headers_end = Some(pos);
                                content_length = parse_content_length(&buf[..pos]).unwrap_or(0);
                            }
                        }
                    }
                }
            }

            let body = if echo_headers {
                let header_text = String::from_utf8_lossy(&buf).to_lowercase();
                serde_json::json!({
                    "choices": [{
                        "message": {"role": "assistant", "content": header_text, "tool_calls": []},
                        "finish_reason": "stop"
                    }]
                })
                .to_string()
            } else {
                static_body
            };

            let mut response = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                status_line,
                body.len()
            );
            if let Some(secs) = retry_after {
                response.push_str(&format!("Retry-After: {secs}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&body);
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }

    fn find_headers_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn parse_content_length(headers: &[u8]) -> Option<usize> {
        let text = String::from_utf8_lossy(headers);
        for line in text.lines() {
            if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                return value.trim().parse::<usize>().ok();
            }
        }
        None
    }
}
