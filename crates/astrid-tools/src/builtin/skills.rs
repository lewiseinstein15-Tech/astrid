//! Skills tools — progressive disclosure: list gives metadata, load fetches
//! one full body on demand.

use crate::registry::{Tool, ToolCtx, ToolOutput};
use astrid_core::{Result, Risk};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::req_str;

pub struct SkillsList;

#[async_trait]
impl Tool for SkillsList {
    fn name(&self) -> &str {
        "skills_list"
    }
    fn description(&self) -> &str {
        "List available skills with name, description and when_to_use — the cheap first look."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "required": []})
    }
    async fn execute(&self, _args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let metas = ctx.skills.list();
        if metas.is_empty() {
            return Ok(ToolOutput {
                content: "no skills available".to_string(),
                meta_json: Some(json!({"count": 0})),
            });
        }
        let lines: Vec<String> = metas
            .iter()
            .map(|meta| match &meta.when_to_use {
                Some(when) => format!("{}: {} (use when: {when})", meta.name, meta.description),
                None => format!("{}: {}", meta.name, meta.description),
            })
            .collect();
        Ok(ToolOutput {
            content: format!("{} skill(s) available:\n{}", metas.len(), lines.join("\n")),
            meta_json: Some(json!({"count": metas.len()})),
        })
    }
}

pub struct SkillsLoad;

#[async_trait]
impl Tool for SkillsLoad {
    fn name(&self) -> &str {
        "skills_load"
    }
    fn description(&self) -> &str {
        "Load one skill's full body by name — progressive disclosure, second step."
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Skill name from skills_list"}
            },
            "required": ["name"]
        })
    }
    async fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<ToolOutput> {
        let name = req_str(self.name(), args, "name")?;
        let body = ctx.skills.body(&name)?;
        let bytes = body.len();
        Ok(ToolOutput {
            content: body,
            meta_json: Some(json!({"name": name, "bytes": bytes})),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_ctx;
    use astrid_core::Error;

    #[tokio::test]
    async fn skills_list_reports_the_bundled_catalog() {
        let (ctx, _rx, root) = make_ctx().await;
        let out = SkillsList.execute(&json!({}), &ctx).await.unwrap();
        let count = out.meta_json.as_ref().unwrap()["count"].as_u64().unwrap();
        assert!(
            count >= 8,
            "bundled catalog expected ≥8 skills, got {count}"
        );
        assert!(out.content.contains("skill(s) available"));
        // Each listed skill carries name + description.
        assert!(out.content.matches(": ").count() >= count as usize);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skills_load_returns_the_full_body() {
        let (ctx, _rx, root) = make_ctx().await;
        let first = ctx
            .skills
            .list()
            .first()
            .expect("bundled skills exist")
            .name
            .clone();
        let out = SkillsLoad
            .execute(&json!({"name": first}), &ctx)
            .await
            .unwrap();
        assert!(!out.content.trim().is_empty(), "body must be non-empty");
        assert_eq!(out.meta_json.as_ref().unwrap()["name"], first);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn missing_name_refused() {
        let (ctx, _rx, root) = make_ctx().await;
        let err = SkillsLoad.execute(&json!({}), &ctx).await.unwrap_err();
        assert!(
            matches!(err, Error::ToolRefused { ref reason, .. } if reason == "missing required arg: name")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
