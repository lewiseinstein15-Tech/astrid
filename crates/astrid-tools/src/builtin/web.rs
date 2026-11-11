//! web_fetch + web_search — the ONLY egress tools (risk External). web_fetch
//! GETs an http(s) URL and strips HTML to plain text with a hand-rolled
//! converter; web_search queries DuckDuckGo Instant Answers and falls back to
//! the Wikipedia search API. Hard caps on size and length; failures are
//! honest errors, never empty successes.

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

// ---------------------------------------------------------------------------
// web_search
// ---------------------------------------------------------------------------

const SEARCH_TIMEOUT_SECS: u64 = 10;
const SEARCH_MAX_RESULTS: usize = 8;
const USER_AGENT: &str = "AstridAgentOS/0.1 (TUI-native agent operating system; offline-first)";

/// One search hit: title, snippet (HTML stripped), source URL.
#[derive(Debug, Clone)]
struct SearchHit {
    title: String,
    snippet: String,
    url: String,
}

pub struct WebSearch {
    ddg_base: String,
    wiki_base: String,
    client: OnceLock<reqwest::Client>,
}

impl Default for WebSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearch {
    pub fn new() -> Self {
        Self {
            ddg_base: "https://api.duckduckgo.com".to_string(),
            wiki_base: "https://en.wikipedia.org".to_string(),
            client: OnceLock::new(),
        }
    }

    /// Test seam: point both backends at mock servers.
    pub fn with_bases(ddg_base: String, wiki_base: String) -> Self {
        Self {
            ddg_base,
            wiki_base,
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> &reqwest::Client {
        self.client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(SEARCH_TIMEOUT_SECS))
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::limited(3))
                .build()
                .expect("web_search client builds with valid configuration")
        })
    }

    /// DuckDuckGo Instant Answers: keyless JSON, but coverage is uneven —
    /// an empty abstract + no related topics means "no results", not failure.
    async fn try_ddg(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>> {
        let resp = self
            .client()
            .get(&self.ddg_base)
            .query(&[("q", query), ("format", "json"), ("no_html", "1")])
            .send()
            .await
            .map_err(|e| Error::Other(format!("duckduckgo: request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Other(format!(
                "duckduckgo: HTTP {}",
                status.as_u16()
            )));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| Error::Other(format!("duckduckgo: bad json: {e}")))?;
        let mut hits = Vec::new();
        if let Some(text) = body.get("AbstractText").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                let title = body
                    .get("Heading")
                    .and_then(Value::as_str)
                    .unwrap_or(query)
                    .to_string();
                let url = body
                    .get("AbstractURL")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                hits.push(SearchHit {
                    title,
                    snippet: text.to_string(),
                    url,
                });
            }
        }
        if let Some(topics) = body.get("RelatedTopics").and_then(Value::as_array) {
            for t in topics {
                if hits.len() >= max_results {
                    break;
                }
                let (Some(text), Some(url)) = (
                    t.get("Text").and_then(Value::as_str),
                    t.get("FirstURL").and_then(Value::as_str),
                ) else {
                    continue;
                };
                // "Title — snippet" is DDG's combined Text format; split it.
                let (title, snippet) = match text.split_once(" - ") {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (text.to_string(), String::new()),
                };
                hits.push(SearchHit {
                    title,
                    snippet,
                    url: url.to_string(),
                });
            }
        }
        Ok(hits)
    }

    /// Wikipedia search API: structured JSON, keyless, reliable coverage.
    /// Requires a descriptive User-Agent (Wikimedia policy) — set on the client.
    async fn try_wikipedia(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>> {
        let resp = self
            .client()
            .get(format!("{}/w/api.php", self.wiki_base))
            .query(&[
                ("action", "query"),
                ("list", "search"),
                ("format", "json"),
                ("srlimit", &max_results.to_string()),
                ("srsearch", query),
            ])
            .send()
            .await
            .map_err(|e| Error::Other(format!("wikipedia: request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Other(format!(
                "wikipedia: HTTP {}",
                status.as_u16()
            )));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| Error::Other(format!("wikipedia: bad json: {e}")))?;
        let search = body
            .get("query")
            .and_then(|q| q.get("search"))
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Other("wikipedia: response missing query.search".into()))?;
        Ok(search
            .iter()
            .take(max_results)
            .filter_map(|item| {
                let title = item.get("title").and_then(Value::as_str)?.to_string();
                let snippet_html = item.get("snippet").and_then(Value::as_str).unwrap_or("");
                let pageid = item.get("pageid").and_then(Value::as_u64)?;
                Some(SearchHit {
                    title: title.clone(),
                    snippet: html_to_text(snippet_html),
                    url: format!("{}/?curid={pageid}", self.wiki_base),
                })
            })
            .collect())
    }
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web: DuckDuckGo Instant Answers first, Wikipedia search API as \
         fallback. Returns titles, snippets and links (max 8). External egress — \
         approval-gated in the auto profile."
    }
    fn risk(&self) -> Risk {
        Risk::External
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query text"},
                "max_results": {"type": "integer", "description": "1-8 (default 5)"}
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, args: &Value, _ctx: &ToolCtx) -> Result<ToolOutput> {
        let _ = _ctx;
        let query = req_str(self.name(), args, "query")?;
        let max_results = super::opt_usize(self.name(), args, "max_results")?
            .unwrap_or(5)
            .clamp(1, SEARCH_MAX_RESULTS);

        let mut failures: Vec<String> = Vec::new();
        let (backend, hits) = match self.try_ddg(&query, max_results).await {
            Ok(h) if !h.is_empty() => ("duckduckgo", h),
            Ok(_) => {
                match self.try_wikipedia(&query, max_results).await {
                    Ok(h) => ("wikipedia", h),
                    Err(e) => return Err(Error::Other(format!(
                        "web_search: no results from duckduckgo; wikipedia failed: {e}"
                    ))),
                }
            }
            Err(e) => {
                failures.push(e.to_string());
                match self.try_wikipedia(&query, max_results).await {
                    Ok(h) => ("wikipedia", h),
                    Err(e2) => {
                        failures.push(e2.to_string());
                        return Err(Error::Other(format!(
                            "web_search failed — {}; {}",
                            failures[0], failures[1]
                        )));
                    }
                }
            }
        };

        if hits.is_empty() {
            return Err(Error::Other(format!(
                "web_search: no results for '{query}' (backend: {backend})"
            )));
        }

        let mut content = format!(
            "Web search for '{query}' — {} result(s) via {backend}:\n\n",
            hits.len()
        );
        for hit in &hits {
            content.push_str(&format!("- **{}**", hit.title));
            if !hit.snippet.is_empty() {
                content.push_str(&format!(" — {}", hit.snippet));
            }
            content.push('\n');
            if !hit.url.is_empty() {
                content.push_str(&format!("  {}\n", hit.url));
            }
        }
        Ok(ToolOutput {
            content,
            meta_json: Some(json!({
                "query": query,
                "backend": backend,
                "results": hits.len(),
                "max_results": max_results,
            })),
        })
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Serve one fixed JSON body for every request (any path), per backend.
    async fn spawn_json_mock(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (mut rd, mut wr) = tokio::io::split(stream);
                let mut buf = [0u8; 8192];
                // drain the request headers
                loop {
                    match tokio::time::timeout(
                        Duration::from_secs(5),
                        rd.read(&mut buf),
                    )
                    .await
                    {
                        Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
                        Ok(Ok(n)) => {
                            if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    body.len()
                );
                let _ = wr.write_all(head.as_bytes()).await;
                let _ = wr.write_all(body.as_bytes()).await;
                let _ = wr.flush().await;
            }
        });
        format!("http://{addr}")
    }

    async fn ctx_for_tests() -> (ToolCtx, tokio::sync::mpsc::Receiver<astrid_core::Event>, std::path::PathBuf) {
        crate::test_support::make_ctx().await
    }

    #[tokio::test]
    async fn ddg_backend_formats_hits_as_markdown_bullets() {
        let ddg = spawn_json_mock(
            "200 OK",
            r#"{"Heading":"Rust","AbstractText":"A systems programming language.","AbstractURL":"https://example.com/rust","RelatedTopics":[{"Text":"Official site - Learn Rust","FirstURL":"https://rust-lang.org"}]}"#,
        )
        .await;
        let tool = WebSearch::with_bases(ddg.clone(), ddg.clone());
        let (ctx, _rx, _root) = ctx_for_tests().await;
        let out = tool
            .execute(&json!({"query": "rust"}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("via duckduckgo"), "got: {}", out.content);
        assert!(out.content.contains("- **Rust** — A systems programming language."), "got: {}", out.content);
        assert!(out.content.contains("https://rust-lang.org"), "got: {}", out.content);
        assert_eq!(out.meta_json.unwrap()["backend"], "duckduckgo");
    }

    #[tokio::test]
    async fn falls_back_to_wikipedia_when_ddg_has_no_results() {
        let ddg = spawn_json_mock("200 OK", r#"{"AbstractText":""}"#).await;
        let wiki = spawn_json_mock(
            "200 OK",
            r#"{"query":{"search":[{"title":"Rust (programming language)","snippet":"<span class=\"searchmatch\">Rust</span> is a multi-paradigm language","pageid":29414838,"wordcount":11088}]}}"#,
        )
        .await;
        let tool = WebSearch::with_bases(ddg, wiki);
        let (ctx, _rx, _root) = ctx_for_tests().await;
        let out = tool
            .execute(&json!({"query": "rust language", "max_results": 2}), &ctx)
            .await
            .unwrap();
        assert!(out.content.contains("via wikipedia"), "got: {}", out.content);
        assert!(out.content.contains("**Rust (programming language)**"), "got: {}", out.content);
        // snippet HTML was stripped
        assert!(out.content.contains("Rust is a multi-paradigm language"), "got: {}", out.content);
        assert!(!out.content.contains("searchmatch"), "got: {}", out.content);
        assert!(out.content.contains("/?curid=29414838"), "got: {}", out.content);
    }

    #[tokio::test]
    async fn both_backends_failing_is_an_honest_error() {
        let ddg = spawn_json_mock("500 Internal Server Error", "boom").await;
        let wiki = spawn_json_mock("500 Internal Server Error", "boom").await;
        let tool = WebSearch::with_bases(ddg, wiki);
        let (ctx, _rx, _root) = ctx_for_tests().await;
        let err = tool
            .execute(&json!({"query": "rust"}), &ctx)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("duckduckgo: HTTP 500"), "got: {msg}");
        assert!(msg.contains("wikipedia: HTTP 500"), "got: {msg}");
    }

    #[tokio::test]
    async fn refusals_on_missing_query() {
        let tool = WebSearch::new();
        let (ctx, _rx, _root) = ctx_for_tests().await;
        let err = tool.execute(&json!({}), &ctx).await.unwrap_err();
        assert!(matches!(err, Error::ToolRefused { .. }), "got: {err}");
    }

    #[test]
    fn schema_requires_query() {
        let tool = WebSearch::new();
        let schema = tool.parameters_schema();
        assert_eq!(schema["required"][0], "query");
        assert_eq!(schema["properties"]["max_results"]["type"], "integer");
        let _ = HashMap::<String, String>::new();
    }
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
