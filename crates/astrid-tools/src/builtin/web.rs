//! web_fetch — the ONLY egress tool (risk External): GET an http(s) URL,
//! strip HTML to plain text with a hand-rolled converter, hard caps on size
//! and length. No search engines, no connectors — v0.1 keeps the dependency
//! tree honest.

use crate::registry::{truncate_chars, Tool, ToolCtx, ToolOutput};
use astrid_core::{Error, Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

use super::req_str;

const FETCH_TIMEOUT_SECS: u64 = 10;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const TEXT_MAX_CHARS: usize = 8000;

pub struct WebFetch;

impl WebFetch {
    fn client() -> &'static reqwest::Client {
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        CLIENT.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
                // ANY redirect is allowed: web_fetch is already External-risk
                // and approval-gated; redirect chains must not surprise.
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    attempt.follow()
                }))
                .build()
                .expect("web_fetch client builds with valid configuration")
        })
    }
}

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn description(&self) -> &str {
        "Fetch an http(s) URL, strip HTML to text (≤2MB body, ≤8000 chars returned). http/https only."
    }
    fn risk(&self) -> Risk {
        Risk::External
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Absolute http:// or https:// URL"}
            },
            "required": ["url"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let _ = ctx;
        let url = req_str(self.name(), args, "url")?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(Error::ToolRefused {
                tool: self.name().to_string(),
                reason: "only http:// and https:// URLs are allowed".to_string(),
            });
        }

        let response = Self::client()
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Other(format!("web_fetch failed for {url}: {e}")))?;
        let status = response.status();
        if let Some(len) = response.content_length() {
            if len as usize > MAX_BODY_BYTES {
                return Err(Error::Other(format!(
                    "web_fetch: {url} body of {len} bytes exceeds the 2MB cap"
                )));
            }
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Other(format!("web_fetch read failed for {url}: {e}")))?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(Error::Other(format!(
                "web_fetch: {url} body of {} bytes exceeds the 2MB cap",
                bytes.len()
            )));
        }
        if !status.is_success() {
            return Err(Error::Other(format!(
                "web_fetch: HTTP {} for {url}",
                status.as_u16()
            )));
        }

        let raw = String::from_utf8_lossy(&bytes);
        let text = html_to_text(&raw);
        let truncated = text.chars().count() > TEXT_MAX_CHARS;
        let content = truncate_chars(text.trim(), TEXT_MAX_CHARS);
        Ok(ToolOutput {
            content: if truncated {
                format!("{content}\n[truncated at {TEXT_MAX_CHARS} chars]")
            } else {
                content
            },
            meta_json: Some(json!({
                "url": url,
                "status": status.as_u16(),
                "bytes": bytes.len(),
                "chars": text.chars().count(),
                "truncated": truncated,
            })),
        })
    }
}

/// Hand-rolled HTML→text: drop script/style blocks, comments and tags,
/// decode the five entities that matter, collapse whitespace.
fn html_to_text(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut cleaned: Vec<u8> = Vec::with_capacity(bytes.len());

    // Pass 1: remove <script>…</script>, <style>…</style> and <!-- … -->.
    let mut i = 0usize;
    while i < bytes.len() {
        if starts_with_ci(bytes, i, b"<script") {
            i = skip_until_ci(bytes, i, b"</script>");
            continue;
        }
        if starts_with_ci(bytes, i, b"<style") {
            i = skip_until_ci(bytes, i, b"</style>");
            continue;
        }
        if starts_with_ci(bytes, i, b"<!--") {
            i = skip_until_ci(bytes, i, b"-->");
            continue;
        }
        cleaned.push(bytes[i]);
        i += 1;
    }

    // Pass 2: tags → space; text kept (byte level so multi-byte UTF-8 text
    // survives; converted via from_utf8_lossy afterwards).
    let mut stripped: Vec<u8> = Vec::with_capacity(cleaned.len());
    let mut in_tag = false;
    for &b in &cleaned {
        match b {
            b'<' => in_tag = true,
            b'>' => {
                in_tag = false;
                stripped.push(b' ');
            }
            _ if !in_tag => stripped.push(b),
            _ => {}
        }
    }
    let text = String::from_utf8_lossy(&stripped).into_owned();

    // Pass 3: decode the entities the docs freeze (amp last, to avoid
    // double-decoding `&amp;lt;`).
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");

    // Pass 4: collapse whitespace runs into single spaces.
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn starts_with_ci(hay: &[u8], at: usize, needle: &[u8]) -> bool {
    at + needle.len() <= hay.len() && hay[at..at + needle.len()].eq_ignore_ascii_case(needle)
}

/// Return the index just past `needle` (or end of input if never found).
fn skip_until_ci(hay: &[u8], from: usize, needle: &[u8]) -> usize {
    let mut i = from;
    while i < hay.len() {
        if starts_with_ci(hay, i, needle) {
            return i + needle.len();
        }
        i += 1;
    }
    hay.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn refuses_non_http_schemes() {
        let (ctx, _rx, _root) = crate::test_support::make_ctx().await;
        for url in ["ftp://example.com/file", "file:///etc/passwd", "gopher://x"] {
            let err = WebFetch
                .execute(&json!({"url": url}), &ctx)
                .await
                .unwrap_err();
            assert!(
                matches!(err, Error::ToolRefused { ref reason, .. } if reason.contains("http")),
                "`{url}` refused: {err}"
            );
        }
    }

    #[test]
    fn html_to_text_strips_scripts_tags_and_decodes_entities() {
        let html = concat!(
            "<html><head><style>body { color: red }</style>",
            "<script>alert('no')</script></head>",
            "<body><h1>Head &amp; Tail</h1>",
            "<p>Compare a &lt; b &gt; c &quot;q&quot; &#39;quote&#39;</p>",
            "<!-- secret comment -->",
            "<script>fetch('steal')</script>",
            "<div>End</div></body></html>"
        );
        let text = html_to_text(html);
        assert!(!text.contains("script"), "got: {text}");
        assert!(!text.contains("alert"), "got: {text}");
        assert!(!text.contains("steal"), "got: {text}");
        assert!(!text.contains("secret comment"), "got: {text}");
        assert!(!text.contains("color: red"), "got: {text}");
        assert!(text.contains("Head & Tail"), "got: {text}");
        assert!(text.contains("a < b > c \"q\" 'quote'"), "got: {text}");
        assert!(text.contains("End"), "got: {text}");
    }

    #[test]
    fn amp_is_decoded_last() {
        assert_eq!(html_to_text("&amp;lt;"), "&lt;");
        assert_eq!(html_to_text("a &amp;&amp; b"), "a && b");
    }
}
