//! Event persistence + fan-out: one pump consumes registry/session events,
//! appends every one to `.astrid/events.jsonl`, and broadcasts to surfaces.

use astrid_core::Event;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};

pub struct EventPump {
    pub tx: mpsc::Sender<Event>,
}

impl EventPump {
    /// Start the pump. Returns the pump handle and a broadcast receiver for
    /// surfaces. `session_started` is emitted before this returns so late
    /// subscribers still see it.
    pub fn start(event_log: PathBuf, session_started: Event) -> (Self, broadcast::Receiver<Event>) {
        let (tx, mut rx) = mpsc::channel::<Event>(512);
        let (btx, brx) = broadcast::channel::<Event>(1024);
        let _ = btx.send(session_started.clone());
        tokio::spawn(async move {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&event_log)
                .ok();
            let _ = append_event(&mut file, &session_started);
            while let Some(ev) = rx.recv().await {
                let _ = btx.send(ev.clone());
                let _ = append_event(&mut file, &ev);
            }
        });
        (Self { tx }, brx)
    }
}

fn append_event(file: &mut Option<std::fs::File>, ev: &Event) -> std::io::Result<()> {
    if let Some(f) = file {
        let mut line = serde_json::to_string(ev).unwrap_or_else(|_| {
            "{\"type\":\"serialize_error\"}".to_string()
        });
        line.push('\n');
        f.write_all(line.as_bytes())?;
        f.flush()?;
    }
    Ok(())
}

/// Headless transcript rendering (ask mode).
pub fn render_event(ev: &Event) -> String {
    match ev {
        Event::TurnStarted { input_preview, .. } => format!("you ▸ {input_preview}"),
        Event::NarrationLine { kind, text, .. } => {
            format!("astrid «{}» {text}", narration_kind(kind))
        }
        Event::MessageDelta { text, .. } => text.clone(),
        Event::ToolStarted { receipt } => format!(
            "▸ {} ({}) {}",
            receipt.tool,
            receipt.risk.as_str(),
            receipt.args_json
        ),
        Event::ToolCompleted { tool, result_preview, duration_ms, .. } => {
            format!("◂ {tool} ok {duration_ms}ms: {result_preview}")
        }
        Event::ToolFailed { tool, error, duration_ms, .. } => {
            format!("✗ {tool} failed {duration_ms}ms: {error}")
        }
        Event::ToolRefused { tool, reason, .. } => format!("⊘ {tool} refused: {reason}"),
        Event::ApprovalRequested { tool, details, .. } => {
            format!("? approval requested: {tool} — {details}")
        }
        Event::ApprovalResolved { approved, .. } => {
            format!("approval {}", if *approved { "granted" } else { "denied" })
        }
        Event::HookFired { event, script, decision } => {
            format!("hook {event} {script} → {decision}")
        }
        Event::TurnCompleted { outcome, provider, ladder_slid, model_calls, .. } => format!(
            "── turn completed: {} ({provider}, slid {ladder_slid}, {model_calls} model calls) ──",
            outcome_text(outcome)
        ),
        Event::SessionStarted { session_id, .. } => format!("session started: {session_id}"),
        Event::SessionEnded { .. } => "session ended".to_string(),
        Event::ThinkingDelta { .. } => String::new(),
    }
}

fn narration_kind(kind: &astrid_core::NarrationKind) -> &'static str {
    match kind {
        astrid_core::NarrationKind::Ack => "ack",
        astrid_core::NarrationKind::Finding => "finding",
        astrid_core::NarrationKind::Progress => "progress",
        astrid_core::NarrationKind::Decision => "decision",
        astrid_core::NarrationKind::Completion => "completion",
    }
}

pub fn outcome_text(o: &astrid_core::TurnOutcome) -> String {
    match o {
        astrid_core::TurnOutcome::Completed => "completed".into(),
        astrid_core::TurnOutcome::CompletedFastPath => "completed (fast path)".into(),
        astrid_core::TurnOutcome::AwaitingApproval => "awaiting approval".into(),
        astrid_core::TurnOutcome::Cancelled => "cancelled".into(),
        astrid_core::TurnOutcome::Failed(r) => format!("failed: {r}"),
        astrid_core::TurnOutcome::Unverified => "unverified".into(),
    }
}

/// True when a turn has reached its terminal event.
pub fn is_turn_end(ev: &Event) -> bool {
    matches!(ev, Event::TurnCompleted { .. })
}
