//! The ratatui application: transcript pane, status rail, input box.

use crate::bridge::ApprovalBridge;
use astrid_core::{Event, TurnOutcome};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// Static facts rendered in the right rail.
#[derive(Clone)]
pub struct TuiStatus {
    pub agent_name: String,
    pub session_id: String,
    pub provider_desc: String,
    pub profile: String,
    pub tool_names: Vec<String>,
    pub skill_count: usize,
    pub agent_count: usize,
    pub memory_pages: usize,
    pub data_dir: String,
    pub hook_count: usize,
}

enum UiKey {
    Char(char),
    Enter,
    Backspace,
    CtrlC,
    Esc,
    PageUp,
    PageDown,
    Other,
}

struct Model {
    status: TuiStatus,
    lines: Vec<ratatui::text::Line<'static>>,
    scroll: usize,
    input: String,
    pending_approval: Option<(String, String)>,
    turn_active: bool,
    last_outcome: Option<String>,
    model_calls_total: usize,
}

impl Model {
    fn push(&mut self, line: ratatui::text::Line<'static>) {
        self.lines.push(line);
        self.scroll = 0; // follow the tail
    }

    fn info(&mut self, text: impl Into<String>) {
        let style = ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray);
        self.push(ratatui::text::Line::styled(text.into(), style));
    }

    fn notice(&mut self, text: impl Into<String>) {
        let style = ratatui::style::Style::default()
            .fg(ratatui::style::Color::Cyan)
            .add_modifier(ratatui::style::Modifier::ITALIC);
        self.push(ratatui::text::Line::styled(text.into(), style));
    }
}

/// Run the TUI until the session closes the input channel or the user quits.
pub async fn run_app(
    mut events: broadcast::Receiver<Event>,
    input_tx: mpsc::Sender<String>,
    approvals: Arc<ApprovalBridge>,
    cancel: Arc<AtomicBool>,
    status: TuiStatus,
) -> astrid_core::Result<()> {
    let mut terminal = ratatui::init();
    let res = app_loop(
        &mut terminal,
        &mut events,
        input_tx,
        approvals,
        cancel,
        status,
    )
    .await;
    ratatui::restore();
    res
}

async fn app_loop(
    terminal: &mut ratatui::DefaultTerminal,
    events: &mut broadcast::Receiver<Event>,
    input_tx: mpsc::Sender<String>,
    approvals: Arc<ApprovalBridge>,
    cancel: Arc<AtomicBool>,
    status: TuiStatus,
) -> astrid_core::Result<()> {
    let (key_tx, mut key_rx) = mpsc::channel::<UiKey>(64);
    spawn_input_thread(key_tx);

    let mut m = Model {
        status,
        lines: Vec::new(),
        scroll: 0,
        input: String::new(),
        pending_approval: None,
        turn_active: false,
        last_outcome: None,
        model_calls_total: 0,
    };
    m.info(format!(
        "Astrid session {} — data in {} — type /help for commands",
        m.status.session_id, m.status.data_dir
    ));

    loop {
        tokio::select! {
            key = key_rx.recv() => {
                match key {
                    None | Some(UiKey::Other) => {}
                    Some(k) => {
                        let quit = handle_key(k, &mut m, &input_tx, &approvals, &cancel).await;
                        if quit {
                            return Ok(());
                        }
                    }
                }
            }
            ev = events.recv() => {
                match ev {
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        m.info(format!("[transcript dropped {n} events — see events.jsonl]"));
                    }
                    Err(_) => return Ok(()), // session gone
                    Ok(event) => {
                        if apply_event(&mut m, event) {
                            return Ok(());
                        }
                    }
                }
            }
        }
        terminal
            .draw(|f| ui(f, &m))
            .map_err(|e| astrid_core::Error::Io(std::io::Error::other(e)))?;
    }
}

/// Apply an event to the model. Returns true when the session has ended.
fn apply_event(m: &mut Model, ev: Event) -> bool {
    match ev {
        Event::SessionStarted { session_id, .. } => {
            m.info(format!("session started: {session_id}"));
            false
        }
        Event::SessionEnded { .. } => true,
        Event::TurnStarted { input_preview, .. } => {
            m.turn_active = true;
            let style = ratatui::style::Style::default()
                .fg(ratatui::style::Color::Green)
                .add_modifier(ratatui::style::Modifier::BOLD);
            m.push(ratatui::text::Line::styled(
                format!("you ▸ {input_preview}"),
                style,
            ));
            false
        }
        Event::MessageDelta { text, .. } => {
            let style = ratatui::style::Style::default().fg(ratatui::style::Color::White);
            m.push(ratatui::text::Line::styled(text, style));
            false
        }
        Event::ThinkingDelta { .. } => false, // not emitted in v1 (no reasoning streams)
        Event::NarrationLine { kind, text, .. } => {
            m.notice(format!("astrid «{}» {text}", kind_str(kind)));
            false
        }
        Event::ToolStarted { receipt } => {
            let style = ratatui::style::Style::default().fg(ratatui::style::Color::Blue);
            m.push(ratatui::text::Line::styled(
                format!(
                    "▸ {} ({}) {}",
                    receipt.tool,
                    receipt.risk.as_str(),
                    truncate(&receipt.args_json, 140)
                ),
                style,
            ));
            false
        }
        Event::ToolCompleted { tool, result_preview, duration_ms, .. } => {
            let style = ratatui::style::Style::default().fg(ratatui::style::Color::Green);
            m.push(ratatui::text::Line::styled(
                format!("◂ {tool} ok {duration_ms}ms: {}", truncate(&result_preview, 160)),
                style,
            ));
            false
        }
        Event::ToolFailed { tool, error, duration_ms, .. } => {
            let style = ratatui::style::Style::default().fg(ratatui::style::Color::Red);
            m.push(ratatui::text::Line::styled(
                format!("✗ {tool} failed {duration_ms}ms: {}", truncate(&error, 160)),
                style,
            ));
            false
        }
        Event::ToolRefused { tool, reason, .. } => {
            let style = ratatui::style::Style::default().fg(ratatui::style::Color::Yellow);
            m.push(ratatui::text::Line::styled(
                format!("⊘ {tool} refused: {reason}"),
                style,
            ));
            false
        }
        Event::ApprovalRequested { tool, details, .. } => {
            m.pending_approval = Some((tool.clone(), details.clone()));
            let style = ratatui::style::Style::default()
                .fg(ratatui::style::Color::Magenta)
                .add_modifier(ratatui::style::Modifier::BOLD);
            m.push(ratatui::text::Line::styled(
                format!("? APPROVAL: {tool} — y approve / n deny (details in right rail)"),
                style,
            ));
            false
        }
        Event::ApprovalResolved { approved, .. } => {
            m.pending_approval = None;
            m.info(format!(
                "approval {}",
                if approved { "granted" } else { "denied" }
            ));
            false
        }
        Event::HookFired { event, script, decision } => {
            m.info(format!("hook {event} {script} → {decision}"));
            false
        }
        Event::TurnCompleted { outcome, provider, ladder_slid, model_calls, .. } => {
            m.turn_active = false;
            m.last_outcome = Some(outcome_label(&outcome));
            m.model_calls_total += model_calls;
            let slide = if ladder_slid > 0 {
                format!(", ladder slid {ladder_slid}")
            } else {
                String::new()
            };
            m.info(format!(
                "── turn completed: {} ({provider}{slide}, {model_calls} model calls) ──",
                outcome_label(&outcome)
            ));
            false
        }
    }
}

fn kind_str(kind: astrid_core::NarrationKind) -> &'static str {
    match kind {
        astrid_core::NarrationKind::Ack => "ack",
        astrid_core::NarrationKind::Finding => "finding",
        astrid_core::NarrationKind::Progress => "progress",
        astrid_core::NarrationKind::Decision => "decision",
        astrid_core::NarrationKind::Completion => "completion",
    }
}

fn outcome_label(o: &TurnOutcome) -> String {
    match o {
        TurnOutcome::Completed => "completed".into(),
        TurnOutcome::CompletedFastPath => "completed (fast path, 0 model calls)".into(),
        TurnOutcome::AwaitingApproval => "awaiting approval".into(),
        TurnOutcome::Cancelled => "cancelled".into(),
        TurnOutcome::Failed(r) => format!("failed: {r}"),
        TurnOutcome::Unverified => "unverified".into(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Handle one key. Returns true when the app should quit.
async fn handle_key(
    k: UiKey,
    m: &mut Model,
    input_tx: &mpsc::Sender<String>,
    approvals: &Arc<ApprovalBridge>,
    cancel: &Arc<AtomicBool>,
) -> bool {
    match k {
        UiKey::CtrlC => {
            if m.pending_approval.is_some() {
                approvals.respond(false);
                return false;
            }
            if m.turn_active {
                cancel.store(true, Ordering::SeqCst);
                m.notice("cancel requested…");
                return false;
            }
            if m.input.is_empty() {
                return true;
            }
            m.input.clear();
            false
        }
        UiKey::Esc => {
            if m.pending_approval.is_some() {
                approvals.respond(false);
            } else {
                m.input.clear();
            }
            false
        }
        UiKey::PageUp => {
            m.scroll = m.scroll.saturating_add(5);
            false
        }
        UiKey::PageDown => {
            m.scroll = m.scroll.saturating_sub(5);
            false
        }
        UiKey::Backspace => {
            m.input.pop();
            false
        }
        UiKey::Enter => {
            let text = m.input.trim().to_string();
            if text.is_empty() {
                return false;
            }
            m.input.clear();
            let _ = input_tx.send(text).await;
            false
        }
        UiKey::Char(c) => {
            // single-key answers while an approval is pending
            if m.pending_approval.is_some() && (c == 'y' || c == 'n') {
                approvals.respond(c == 'y');
                return false;
            }
            m.input.push(c);
            false
        }
        UiKey::Other => false,
    }
}

/// Blocking crossterm reader on a dedicated thread, forwarded over tokio.
fn spawn_input_thread(key_tx: mpsc::Sender<UiKey>) {
    std::thread::spawn(move || {
        use crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers};
        loop {
            match event::read() {
                Ok(TermEvent::Key(k)) => {
                    let key = match (k.code, k.modifiers) {
                        (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => UiKey::CtrlC,
                        (KeyCode::Char('y'), m) if m.contains(KeyModifiers::CONTROL) => UiKey::Other,
                        (KeyCode::Char(c), _) => UiKey::Char(c),
                        (KeyCode::Enter, _) => UiKey::Enter,
                        (KeyCode::Backspace, _) => UiKey::Backspace,
                        (KeyCode::Esc, _) => UiKey::Esc,
                        (KeyCode::PageUp, _) => UiKey::PageUp,
                        (KeyCode::PageDown, _) => UiKey::PageDown,
                        _ => UiKey::Other,
                    };
                    if key_tx.blocking_send(key).is_err() {
                        return; // app loop gone
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

fn ui(f: &mut ratatui::Frame, m: &Model) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span, Text};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let rows = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(f.area());
    let cols = Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(rows[0]);

    // transcript
    let transcript = Paragraph::new(Text::from(m.lines.clone()))
        .block(Block::new().borders(Borders::ALL).title(" transcript (pgup/pgdn) "))
        .wrap(ratatui::widgets::Wrap { trim: false })
        .scroll((m.scroll as u16, 0));
    f.render_widget(transcript, cols[0]);

    // right rail
    let mut rail: Vec<Line> = Vec::new();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    rail.push(Line::styled(m.status.agent_name.clone(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
    rail.push(Line::from(format!("session  {}", m.status.session_id)));
    rail.push(Line::from(format!("provider {}", m.status.provider_desc)));
    rail.push(Line::from(format!("profile  {}", m.status.profile)));
    rail.push(Line::from(format!(
        "agents   {} · skills {} · hooks {}",
        m.status.agent_count, m.status.skill_count, m.status.hook_count
    )));
    rail.push(Line::from(format!(
        "memory   {} pages · tools {}",
        m.status.memory_pages,
        m.status.tool_names.len()
    )));
    rail.push(Line::from(format!("data     {}", m.status.data_dir)));
    rail.push(Line::from(format!(
        "turns    {}",
        m.last_outcome.as_deref().unwrap_or("—")
    )));
    rail.push(Line::from(""));
    rail.push(Line::styled("tools", bold));
    for t in &m.status.tool_names {
        rail.push(Line::from(format!("  {t}")));
    }
    if let Some((tool, details)) = &m.pending_approval {
        rail.push(Line::from(""));
        rail.push(Line::styled(
            format!("? pending approval: {tool}"),
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ));
        rail.push(Line::styled(details.clone(), Style::default().fg(Color::Magenta)));
    }
    let rail_view = Paragraph::new(Text::from(rail))
        .block(Block::new().borders(Borders::ALL).title(" status "))
        .wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(rail_view, cols[1]);

    // input box
    let input_title = if m.pending_approval.is_some() {
        " APPROVAL PENDING — y approve · n/esc deny "
    } else if m.turn_active {
        " input — ctrl+c cancels the turn "
    } else {
        " input — enter sends · ctrl+c quits when idle "
    };
    let input = Paragraph::new(format!("› {}", m.input))
        .block(Block::new().borders(Borders::ALL).title(input_title));
    f.render_widget(input, rows[1]);

    // status bar
    let state = if m.pending_approval.is_some() {
        Span::styled(" APPROVAL ", Style::default().fg(Color::Magenta))
    } else if m.turn_active {
        Span::styled(" WORKING ", Style::default().fg(Color::Blue))
    } else {
        Span::styled(" READY ", Style::default().fg(Color::Green))
    };
    let bar = Line::from(vec![
        state,
        Span::raw(format!(
            "  {} · {} model calls this session · ctrl+c quit (idle)",
            m.status.provider_desc, m.model_calls_total
        )),
    ]);
    f.render_widget(paragraph_of_line(bar), rows[2]);
}

fn paragraph_of_line(line: ratatui::text::Line<'static>) -> ratatui::widgets::Paragraph<'static> {
    ratatui::widgets::Paragraph::new(ratatui::text::Text::from(vec![line]))
}
