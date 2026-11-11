//! Honest eval harness: run the real turn loop against task fixtures and
//! grade the real artifacts. `unverified` is never reported as `pass`.

use crate::session::{HeadlessApproval, Session};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct EvalTask {
    pub id: String,
    pub task: String,
    pub check: EvalCheck,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EvalCheck {
    /// The final answer must contain this regex.
    AnswerRegex { pattern: String },
    /// A file inside the task sandbox must contain this string.
    FileContains { path: String, needle: String },
    /// A tool with this name must have been called successfully.
    ToolUsed { tool: String },
}

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub run_id: String,
    pub provider: String,
    pub tasks: Vec<EvalTaskResult>,
    pub summary: EvalSummary,
}

#[derive(Debug, Serialize)]
pub struct EvalTaskResult {
    pub id: String,
    pub outcome: String,
    pub passed: bool,
    pub note: String,
    pub duration_ms: u128,
}

#[derive(Debug, Serialize)]
pub struct EvalSummary {
    pub passed: usize,
    pub total: usize,
    pub rate: f64,
}

pub async fn run_eval(tasks_path: &Path, out: Option<PathBuf>) -> Result<EvalReport> {
    let raw = std::fs::read_to_string(tasks_path)
        .with_context(|| format!("reading {}", tasks_path.display()))?;
    let file: EvalFile = serde_json::from_str(&raw).context("parsing tasks file")?;
    if file.tasks.is_empty() {
        anyhow::bail!("no tasks in {}", tasks_path.display());
    }

    let run_id = astrid_core::new_id("eval");
    let mut results = Vec::new();
    for task in &file.tasks {
        let started = std::time::Instant::now();
        let r = run_one(task).await;
        results.push(EvalTaskResult {
            id: task.id.clone(),
            outcome: r.0,
            passed: r.1,
            note: r.2,
            duration_ms: started.elapsed().as_millis(),
        });
    }

    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();
    let report = EvalReport {
        run_id,
        provider: results
            .first()
            .map(|_| "see task notes".to_string())
            .unwrap_or_default(),
        tasks: results,
        summary: EvalSummary {
            passed,
            total,
            rate: if total > 0 {
                passed as f64 / total as f64
            } else {
                0.0
            },
        },
    };

    // print table
    println!("{:<18} {:<28} {:<8} note", "task", "outcome", "pass");
    for r in &report.tasks {
        println!(
            "{:<18} {:<28} {:<8} {}",
            r.id,
            truncate(&r.outcome, 26),
            if r.passed { "yes" } else { "no" },
            truncate(&r.note, 80)
        );
    }
    println!(
        "\n{} / {} passed ({}%)",
        report.summary.passed,
        report.summary.total,
        (report.summary.rate * 100.0).round() as i64
    );

    let json = serde_json::to_string_pretty(&report)?;
    let out_path = out.unwrap_or_else(|| {
        PathBuf::from(format!("eval-results-{}.json", chrono::Utc::now().format("%Y%m%d-%H%M%S")))
    });
    std::fs::write(&out_path, &json)
        .with_context(|| format!("writing report to {}", out_path.display()))?;
    println!("report: {}", out_path.display());
    Ok(report)
}

#[derive(Debug, Deserialize)]
struct EvalFile {
    tasks: Vec<EvalTask>,
}

/// Run one task in an isolated sandbox data dir; grade the real artifact.
async fn run_one(task: &EvalTask) -> (String, bool, String) {
    let workdir = std::env::temp_dir().join(format!(
        "astrid-eval-{}-{}",
        task.id,
        astrid_core::new_id("w").chars().take(8).collect::<String>()
    ));
    let _ = std::fs::create_dir_all(&workdir);
    let data_dir = workdir.join(".astrid");

    let cfg = astrid_core::AstridConfig {
        data_dir: data_dir.clone(),
        permissions: astrid_core::Profile::Auto,
        ..astrid_core::AstridConfig::default()
    };
    let result = async {
        let (session, mut rx) = Session::new(
            cfg,
            workdir.clone(),
            Some(HeadlessApproval::Approve), // sandboxed fixture: approvals granted
        )
        .await?;
        // drain events in the background (receipts land in events.jsonl too)
        let drain = tokio::spawn(async move {
            let mut used_tools = Vec::new();
            while let Ok(ev) = rx.recv().await {
                if let astrid_core::Event::ToolCompleted { tool, .. } = ev {
                    used_tools.push(tool);
                }
            }
            used_tools
        });

        let report = session.turn(&task.task).await?;
        session.end().await;
        drop(session); // close the pump so the drain task finishes
        let used_tools = match tokio::time::timeout(std::time::Duration::from_secs(5), drain).await
        {
            Ok(joined) => joined.unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        let outcome = outcome_label(&report.outcome);
        let (passed, note) = match &task.check {
            EvalCheck::AnswerRegex { pattern } => {
                let re = regex::Regex::new(pattern)?;
                match &report.answer {
                    Some(a) if re.is_match(a) => (true, "answer matched".into()),
                    Some(_) => (false, "answer did not match the check".into()),
                    None => (false, "no answer produced".into()),
                }
            }
            EvalCheck::FileContains { path, needle } => {
                let full = workdir.join(path.trim_start_matches('/'));
                match std::fs::read_to_string(&full) {
                    Ok(body) if body.contains(needle) => (true, "file artifact verified".into()),
                    Ok(_) => (false, format!("{} exists but misses the needle", path)),
                    Err(_) => (false, format!("{} not written", path)),
                }
            }
            EvalCheck::ToolUsed { tool } => {
                if used_tools.iter().any(|t| t == tool) {
                    (true, format!("{tool} completed successfully"))
                } else {
                    (false, format!("{tool} was never completed"))
                }
            }
        };
        Ok::<(String, bool, String), anyhow::Error>((outcome, passed, note))
    };

    let value = match result.await {
        Ok(v) => v,
        Err(e) => (format!("error: {e}"), false, e.to_string()),
    };
    let _ = std::fs::remove_dir_all(&workdir);
    value
}

fn outcome_label(o: &astrid_core::TurnOutcome) -> String {
    match o {
        astrid_core::TurnOutcome::Completed => "completed".into(),
        astrid_core::TurnOutcome::CompletedFastPath => "completed-fast-path".into(),
        astrid_core::TurnOutcome::AwaitingApproval => "awaiting-approval".into(),
        astrid_core::TurnOutcome::Cancelled => "cancelled".into(),
        astrid_core::TurnOutcome::Failed(r) => format!("failed: {r}"),
        astrid_core::TurnOutcome::Unverified => "unverified".into(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
