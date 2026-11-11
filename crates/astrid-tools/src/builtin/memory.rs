//! Memory tools — the five-verb protocol surfaces as three tools here
//! (remember / recall / forget; entity and synthesize are exposed via the
//! kernel lane, not the tool menu in v0.1).

use crate::registry::{Tool, ToolCtx, ToolOutput};
use astrid_core::{Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{opt_str, opt_usize, req_str};

const DEFAULT_RECALL_K: usize = 5;

pub struct MemRememberTool;

#[async_trait]
impl Tool for MemRememberTool {
    fn name(&self) -> &str {
        "mem_remember"
    }
    fn description(&self) -> &str {
        "Write/update a memory page: {topic, body, kind?}. Returns the page id."
    }
    fn risk(&self) -> Risk {
        Risk::Medium
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "topic": {"type": "string", "description": "Short topic key for the page"},
                "body": {"type": "string", "description": "Page content (markdown)"},
                "kind": {"type": "string", "description": "Page kind: note | reference | log (default: note)"}
            },
            "required": ["topic", "body"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let topic = req_str(self.name(), args, "topic")?;
        let body = req_str(self.name(), args, "body")?;
        let kind = opt_str(self.name(), args, "kind")?.unwrap_or_else(|| "note".to_string());
        let page_id = ctx.memory.remember(&topic, &body, &kind)?;
        Ok(ToolOutput {
            content: format!("remembered `{topic}` as page {page_id} (kind: {kind})"),
            meta_json: Some(json!({"page_id": page_id, "kind": kind})),
        })
    }
}

pub struct MemRecallTool;

#[async_trait]
impl Tool for MemRecallTool {
    fn name(&self) -> &str {
        "mem_recall"
    }
    fn description(&self) -> &str {
        "Hybrid-search memory pages: {query, k?}. Hits carry scores and per-stage breakdowns."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "k": {"type": "integer", "description": "Max hits (default 5)"}
            },
            "required": ["query"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let query = req_str(self.name(), args, "query")?;
        let k = opt_usize(self.name(), args, "k")?
            .unwrap_or(DEFAULT_RECALL_K)
            .max(1);
        let hits = ctx.memory.recall(&query, k)?;
        if hits.is_empty() {
            return Ok(ToolOutput {
                content: format!("no memory hits for `{query}`"),
                meta_json: Some(json!({"query": query, "hits": 0})),
            });
        }
        let lines: Vec<String> = hits
            .iter()
            .enumerate()
            .map(|(i, hit)| {
                let stages = hit
                    .stages
                    .iter()
                    .map(|(stage, score)| format!("{stage}={score:.3}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{}. {} (score {:.3})\n   id: {}\n   {}\n   stages: {}",
                    i + 1,
                    hit.title,
                    hit.score,
                    hit.id,
                    hit.snippet,
                    stages
                )
            })
            .collect();
        Ok(ToolOutput {
            content: format!(
                "{} memory hit(s) for `{query}`:\n{}",
                hits.len(),
                lines.join("\n")
            ),
            meta_json: Some(json!({"query": query, "hits": hits.len()})),
        })
    }
}

pub struct MemForgetImage;

#[async_trait]
impl Tool for MemForgetImage {
    fn name(&self) -> &str {
        "mem_forget"
    }
    fn description(&self) -> &str {
        "Tombstone a memory page by id, recording why: {id, reason}. Content is never deleted."
    }
    fn risk(&self) -> Risk {
        Risk::Medium
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Page id from mem_remember or mem_recall"},
                "reason": {"type": "string", "description": "Why the page is being forgotten"}
            },
            "required": ["id", "reason"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let id = req_str(self.name(), args, "id")?;
        let reason = req_str(self.name(), args, "reason")?;
        ctx.memory.forget(&id, &reason)?;
        Ok(ToolOutput {
            content: format!("tombstoned {id} — reason: {reason}"),
            meta_json: Some(json!({"id": id, "reason": reason})),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_ctx;
    use astrid_core::Error;

    #[tokio::test]
    async fn remember_then_recall_roundtrip_via_real_kernel() {
        let (ctx, _rx, root) = make_ctx().await;
        let out = MemRememberTool
            .execute(
                &json!({"topic": "provider-ladder", "body": "The provider ladder slides down to the offline agent when a rung fails or cools down."}),
                &ctx,
            )
            .await
            .unwrap();
        let page_id = out.meta_json.as_ref().unwrap()["page_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!page_id.is_empty());

        let hits = MemRecallTool
            .execute(&json!({"query": "provider ladder"}), &ctx)
            .await
            .unwrap();
        assert!(
            !hits.content.contains("no memory hits"),
            "expected a hit, got: {}",
            hits.content
        );
        assert!(
            hits.content.contains("provider-ladder"),
            "got: {}",
            hits.content
        );
        assert!(hits.content.contains("score"), "stage scores must render");

        // k parameter bounds the hit count.
        let one = MemRecallTool
            .execute(&json!({"query": "provider ladder", "k": 1}), &ctx)
            .await
            .unwrap();
        assert!(one.meta_json.as_ref().unwrap()["hits"].as_u64().unwrap() <= 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn forget_tombstones_a_page() {
        let (ctx, _rx, root) = make_ctx().await;
        let out = MemRememberTool
            .execute(
                &json!({"topic": "temp-note", "body": "soon forgotten"}),
                &ctx,
            )
            .await
            .unwrap();
        let page_id = out.meta_json.as_ref().unwrap()["page_id"]
            .as_str()
            .unwrap()
            .to_string();

        let forgotten = MemForgetImage
            .execute(
                &json!({"id": page_id, "reason": "obsolete scratch note"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(
            forgotten.content.contains("tombstoned"),
            "got: {}",
            forgotten.content
        );
        assert!(forgotten.content.contains("obsolete scratch note"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn missing_args_refused() {
        let (ctx, _rx, root) = make_ctx().await;
        let err = MemRememberTool
            .execute(&json!({"topic": "t"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: body")
        );
        let err = MemRecallTool.execute(&json!({}), &ctx).await.unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: query")
        );
        let err = MemForgetImage
            .execute(&json!({"id": "x"}), &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: reason")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
