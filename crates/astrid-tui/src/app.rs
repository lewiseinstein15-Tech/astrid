//! The Agent OS surface: a themed, streaming transcript with receipt cards,
//! a capability rail (providers / tools by category / MCP / plugins), live
//! approval flow, and an input loop. One hue per role — see `theme.rs`.

use crate::bridge::ApprovalBridge;
use crate::theme::{self, Palette};
use astrid_core::{Event, NarrationKind, TurnOutcome};
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
    /// Startup theme name (from config; `/theme` switches live).
    pub theme: String,
    /// One line per provider rung: `id: state`.
    pub ladder_summary: Vec<String>,
    /// One line per MCP server: connection state + tool count.
    pub mcp_lines: Vec<String>,
    /// Number of loaded manifest plugins.
    pub plugin_count: usize,
}

enum UiKey {
    Char(char),
    Enter,
    Backspace,
    CtrlC,
    Esc,
    PageUp,
    PageDown,
    End,
    Other,
}

const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

struct Model {
    status: TuiStatus,
    pal: Palette,
    /// Completed transcript lines (static history).
    lines: Vec<ratatui::text::Line<'static>>,
    /// In-flight assistant streaming buffer (None when idle).
    stream_buf: String,
    streaming: bool,
    /// In-flight reasoning buffer.
    thinking_buf: String,
    /// Scroll offset from the top of the (wrapped) transcript.
    scroll: usize,
    /// Follow the newest lines automatically; disabled by PageUp.
    follow_tail: bool,
    ticks: usize,
    turn_active: bool,
    input: String,
    pending_approval: Option<(String, String)>,
    last_outcome: Option<String>,
    model_calls_total: usize,
    tool_calls_total: usize,
}

impl Model {
    fn new(status: TuiStatus) -> Self {
        let pal = theme::by_name(&status.theme);
        Self {
            status,
            pal,
            lines: Vec::new(),
            stream_buf: String::new(),
            streaming: false,
            thinking_buf: String::new(),
            scroll: 0,
            follow_tail: true,
            ticks: 0,
            turn_active: false,
            input: String::new(),
            pending_approval: None,
            last_outcome: None,
            model_calls_total: 0,
            tool_calls_total: 0,
        }
    }

    fn push(&mut self, line: ratatui::text::Line<'static>) {
        self.lines.push(line);
    }

    fn info(&mut self, text: impl Into<String>) {
        self.push(ratatui::text::Line::styled(text.into(), self.pal.dim()));
    }

    /// Append streamed content, splitting on newlines into finished lines.
    fn stream_append(&mut self, text: &str) {
        self.streaming = true;
        self.thinking_buf.clear();
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            self.stream_buf.push_str(part);
            if parts.peek().is_some() {
                let done = std::mem::take(&mut self.stream_buf);
                let style = self.pal.italic(self.pal.assistant);
                self.push(ratatui::text::Line::styled(done, style));
            }
        }
    }

    /// Close the streaming buffer into the transcript.
    fn flush_stream(&mut self) {
        if !self.stream_buf.is_empty() {
            let done = std::mem::take(&mut self.stream_buf);
            let style = self.pal.italic(self.pal.assistant);
            self.push(ratatui::text::Line::styled(done, style));
        }
        self.streaming = false;
        self.thinking_buf.clear();
    }

    fn narrate(&mut self, kind: NarrationKind, text: &str) {
        let (icon, color) = match kind {
            NarrationKind::Ack => ("✦", self.pal.ack),
            NarrationKind::Progress => ("»", self.pal.progress),
            NarrationKind::Finding => ("◈", self.pal.finding),
            NarrationKind::Decision => ("➤", self.pal.decision),
            NarrationKind::Completion => ("✓", self.pal.completion),
        };
        let mut spans = Vec::new();
        spans.push(ratatui::text::Span::styled(
            format!("{icon} "),
            self.pal.bold(color),
        ));
        spans.push(ratatui::text::Span::styled("astrid ", self.pal.dim()));
        spans.push(ratatui::text::Span::styled(
            text.to_string(),
            self.pal.italic(color),
        ));
        self.push(ratatui::text::Line::from(spans));
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

    let mut m = Model::new(status);
    m.info(format!(
        "Astrid Agent OS session {} — data in {} — /help for commands, /theme for palettes",
        m.status.session_id, m.status.data_dir
    ));

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(120));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = tick.tick() => {
                m.ticks = m.ticks.wrapping_add(1);
            }
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
            .draw(|f| ui(f, &mut m))
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
        Event::SessionEnded { .. } => {
            m.flush_stream();
            true
        }
        Event::TurnStarted { input_preview, .. } => {
            m.flush_stream();
            m.turn_active = true;
            m.follow_tail = true;
            m.push(ratatui::text::Line::styled(
                format!("you ▸ {input_preview}"),
                m.pal.bold(m.pal.user),
            ));
            false
        }
        Event::MessageDelta { text, .. } => {
            m.stream_append(&text);
            false
        }
        Event::ThinkingDelta { text, .. } => {
            m.thinking_buf.push_str(&text);
            false
        }
        Event::NarrationLine { kind, text, .. } => {
            m.narrate(kind, &text);
            false
        }
        Event::ToolStarted { receipt } => {
            m.tool_calls_total += 1;
            let c = m.pal.tool_color(&receipt.tool);
            let mut spans = Vec::new();
            spans.push(ratatui::text::Span::styled("▸ ", m.pal.bold(c)));
            spans.push(ratatui::text::Span::styled(receipt.tool.clone(), m.pal.bold(c)));
            spans.push(ratatui::text::Span::styled(
                format!(" ·{}· ", receipt.risk.as_str()),
                m.pal.dim(),
            ));
            spans.push(ratatui::text::Span::styled(
                truncate(&receipt.args_json, 140),
                m.pal.dim(),
            ));
            m.push(ratatui::text::Line::from(spans));
            false
        }
        Event::ToolCompleted { tool, result_preview, duration_ms, .. } => {
            let c = m.pal.tool_color(&tool);
            let mut spans = Vec::new();
            spans.push(ratatui::text::Span::styled("◂ ", m.pal.bold(m.pal.ok)));
            spans.push(ratatui::text::Span::styled(
                format!("ok {duration_ms}ms "),
                m.pal.fg(m.pal.ok),
            ));
            spans.push(ratatui::text::Span::styled(tool.clone(), m.pal.bold(c)));
            spans.push(ratatui::text::Span::styled(
                format!(" — {}", truncate(&result_preview, 160)),
                m.pal.dim(),
            ));
            m.push(ratatui::text::Line::from(spans));
            false
        }
        Event::ToolFailed { tool, error, duration_ms, .. } => {
            m.push(ratatui::text::Line::styled(
                format!("✗ {tool} failed {duration_ms}ms — {}", truncate(&error, 160)),
                m.pal.fg(m.pal.fail),
            ));
            false
        }
        Event::ToolRefused { tool, reason, .. } => {
            m.push(ratatui::text::Line::styled(
                format!("⊘ {tool} refused — {reason}"),
                m.pal.bold(m.pal.refused),
            ));
            false
        }
        Event::ApprovalRequested { tool, details, .. } => {
            m.pending_approval = Some((tool.clone(), details.clone()));
            m.push(ratatui::text::Line::styled(
                format!("? approval needed: {tool} — y approve · n/esc deny"),
                m.pal.bold(m.pal.approval),
            ));
            false
        }
        Event::ApprovalResolved { approved, .. } => {
            m.pending_approval = None;
            m.info(format!(
                "approval {}",
                if approved { "granted ✓" } else { "denied ⊘" }
            ));
            false
        }
        Event::HookFired { event, script, decision } => {
            m.info(format!("hook {event} {script} → {decision}"));
            false
        }
        Event::TurnCompleted { outcome, provider, ladder_slid, model_calls, .. } => {
            m.turn_active = false;
            m.flush_stream();
            m.last_outcome = Some(outcome_label(&outcome));
            m.model_calls_total += model_calls;
            let slide = if ladder_slid > 0 {
                format!(" · ladder slid {ladder_slid}")
            } else {
                String::new()
            };
            m.info(format!(
                "── {} · {provider}{slide} · {model_calls} model calls ──",
                outcome_label(&outcome)
            ));
            false
        }
    }
}

fn outcome_label(o: &TurnOutcome) -> String {
    match o {
        TurnOutcome::Completed => "completed".into(),
        TurnOutcome::CompletedFastPath => "completed (fast path)".into(),
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
                m.narrate(astrid_core::NarrationKind::Progress, "cancel requested…");
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
            m.follow_tail = false;
            m.scroll = m.scroll.saturating_sub(10);
            false
        }
        UiKey::PageDown => {
            m.scroll = m.scroll.saturating_add(10);
            false
        }
        UiKey::End => {
            m.follow_tail = true;
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
            // TUI-local commands never reach the session.
            if let Some(rest) = text.strip_prefix("/theme") {
                let arg = rest.trim();
                if arg.is_empty() {
                    m.info(format!(
                        "themes: {} (usage: /theme <name>)",
                        theme::THEMES.join(", ")
                    ));
                } else {
                    m.pal = theme::by_name(arg);
                    m.info(format!("theme switched to {}", m.pal.name));
                }
                return false;
            }
            if text == "/clear" {
                m.lines.clear();
                m.info("transcript cleared (receipts kept in events.jsonl)");
                return false;
            }
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
                        (KeyCode::End, _) => UiKey::End,
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

/// The dynamic tail of the transcript: reasoning indicator + live stream.
fn display_lines(m: &Model) -> Vec<ratatui::text::Line<'static>> {
    let mut out = m.lines.clone();
    if !m.thinking_buf.is_empty() {
        let head = truncate(&m.thinking_buf, 400);
        out.push(ratatui::text::Line::styled(
            format!("◌ thinking… {head}"),
            m.pal.italic(m.pal.dim),
        ));
    }
    if m.streaming || !m.stream_buf.is_empty() {
        out.push(ratatui::text::Line::styled(
            format!("▍{}", m.stream_buf),
            m.pal.italic(m.pal.assistant),
        ));
    }
    out
}

/// Rough wrapped height of the transcript at the given pane width. Ratatui
/// wraps on word boundaries; ceil(width/pane) is close enough for smooth
/// tail-following.
fn wrapped_height(lines: &[ratatui::text::Line], width: u16) -> usize {
    if width == 0 {
        return lines.len();
    }
    let w = width as usize;
    lines
        .iter()
        .map(|l| l.width().div_ceil(w).max(1))
        .sum()
}

fn ui(f: &mut ratatui::Frame, m: &mut Model) {
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span, Text};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(f.area());
    let cols = Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(rows[1]);

    // ---- header bar ----
    let spinner = SPINNER[m.ticks % SPINNER.len()];
    let header = Line::from(vec![
        Span::styled("◆ ", Style::default().fg(m.pal.accent)),
        Span::styled("ASTRID AGENT OS", Style::default().fg(m.pal.accent).add_modifier(Modifier::BOLD)),
        Span::styled("  ·  ", m.pal.dim()),
        Span::styled(format!("theme {}", m.pal.name), m.pal.italic(m.pal.accent)),
        Span::styled("  ·  ", m.pal.dim()),
        Span::styled(m.status.provider_desc.clone(), m.pal.fg(m.pal.info)),
        Span::styled("  ·  ", m.pal.dim()),
        Span::styled(
            format!("{} {}", spinner, m.status.agent_name),
            m.pal.italic(m.pal.info),
        ),
    ]);
    f.render_widget(Paragraph::new(header), rows[0]);

    // ---- transcript (tail-following) ----
    let content = display_lines(m);
    let pane_w = cols[0].width.saturating_sub(2);
    let pane_h = cols[0].height.saturating_sub(2) as usize;
    let total_h = wrapped_height(&content, pane_w);
    let max_scroll = total_h.saturating_sub(pane_h);
    if m.follow_tail || m.scroll >= max_scroll {
        m.follow_tail = true;
        m.scroll = max_scroll;
    }
    let title = if m.follow_tail {
        " transcript — live (pgup to scroll) "
    } else {
        " transcript — scrolled (pgdn/end to follow) "
    };
    let transcript = Paragraph::new(Text::from(content))
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(m.pal.border))
                .title(Span::styled(title, Style::default().fg(m.pal.border_focus))),
        )
        .wrap(ratatui::widgets::Wrap { trim: false })
        .scroll((m.scroll as u16, 0));
    f.render_widget(transcript, cols[0]);

    // ---- right rail ----
    let mut rail: Vec<Line> = Vec::new();
    let section = |text: &str, m: &Model| {
        Line::from(Span::styled(
            format!("── {text} ──"),
            Style::default().fg(m.pal.accent).add_modifier(Modifier::BOLD),
        ))
    };
    rail.push(section("session", m));
    rail.push(Line::styled(
        format!("id      {}", truncate(&m.status.session_id, 26)),
        m.pal.fg(m.pal.info),
    ));
    rail.push(Line::styled(
        format!("data    {}", m.status.data_dir),
        m.pal.fg(m.pal.info),
    ));
    rail.push(Line::styled(
        format!("profile {}", m.status.profile),
        m.pal.fg(m.pal.info),
    ));

    rail.push(section("providers", m));
    for rung in &m.status.ladder_summary {
        let (name, state) = rung.split_once(": ").unwrap_or((rung.as_str(), "?"));
        let state_color = match state {
            "configured" => m.pal.ok,
            "unconfigured" => m.pal.dim,
            _ => m.pal.refused,
        };
        rail.push(Line::from(vec![
            Span::styled("  ● ", Style::default().fg(state_color)),
            Span::styled(name.to_string(), m.pal.fg(m.pal.assistant)),
            Span::styled(format!(" {state}"), m.pal.fg(state_color)),
        ]));
    }

    rail.push(section("capabilities", m));
    rail.push(Line::styled(
        format!(
            "agents {} · skills {} · hooks {}",
            m.status.agent_count, m.status.skill_count, m.status.hook_count
        ),
        m.pal.fg(m.pal.info),
    ));
    rail.push(Line::styled(
        format!(
            "memory {} pages · turns {}",
            m.status.memory_pages,
            m.last_outcome.as_deref().unwrap_or("—")
        ),
        m.pal.fg(m.pal.info),
    ));

    rail.push(section("tools by family", m));
    for (family, tools) in grouped_tools(&m.status.tool_names) {
        let color = m.pal.tool_color(&tools[0]);
        rail.push(Line::from(vec![
            Span::styled(format!("  ● "), Style::default().fg(color)),
            Span::styled(
                format!("{family} ({})", tools.len()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {}", truncate(&tools.join(", "), 120)),
                m.pal.dim(),
            ),
        ]));
    }

    if !m.status.mcp_lines.is_empty() {
        rail.push(section("mcp servers", m));
        for l in &m.status.mcp_lines {
            let color = if l.contains("✓") {
                m.pal.tool_mcp
            } else {
                m.pal.refused
            };
            rail.push(Line::from(vec![
                Span::styled("  ● ", Style::default().fg(color)),
                Span::styled(l.clone(), m.pal.fg(m.pal.info)),
            ]));
        }
    }
    if m.status.plugin_count > 0 {
        rail.push(section("plugins", m));
        rail.push(Line::styled(
            format!("  {} loaded from plugins/", m.status.plugin_count),
            m.pal.fg(m.pal.tool_plugin),
        ));
    }

    if let Some((tool, details)) = &m.pending_approval {
        rail.push(Line::from(""));
        rail.push(Line::styled(
            format!("? APPROVAL: {tool}"),
            Style::default()
                .fg(m.pal.approval)
                .add_modifier(Modifier::BOLD),
        ));
        rail.push(Line::styled(details.clone(), m.pal.fg(m.pal.approval)));
        rail.push(Line::styled("  y approve · n/esc deny", m.pal.dim()));
    }

    let rail_view = Paragraph::new(Text::from(rail))
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(m.pal.border))
                .title(Span::styled(" status ", Style::default().fg(m.pal.accent))),
        )
        .wrap(ratatui::widgets::Wrap { trim: false });
    f.render_widget(rail_view, cols[1]);

    // ---- input box ----
    let (input_title, border_color) = if m.pending_approval.is_some() {
        (" APPROVAL PENDING — y approve · n/esc deny ", m.pal.approval)
    } else if m.turn_active {
        (" input — ctrl+c cancels the turn ", m.pal.border_focus)
    } else {
        (" input — enter sends · ctrl+c quits when idle ", m.pal.border)
    };
    let body = if m.input.is_empty() && m.pending_approval.is_none() && !m.turn_active {
        Span::styled("› ask anything — /help for commands", m.pal.dim())
    } else {
        Span::styled(format!("› {}", m.input), m.pal.fg(m.pal.assistant))
    };
    let input = Paragraph::new(Line::from(body)).block(
        Block::new()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(
                input_title,
                Style::default().fg(border_color),
            )),
    );
    f.render_widget(input, rows[2]);

    // ---- status bar ----
    let spinner_span = |m: &Model, color| {
        Span::styled(
            format!(" {} ", SPINNER[m.ticks % SPINNER.len()]),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    };
    let state_span = if m.pending_approval.is_some() {
        Line::from(vec![
            spinner_span(m, m.pal.status_approval),
            Span::styled(
                "APPROVAL ",
                Style::default()
                    .fg(m.pal.status_approval)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else if m.streaming {
        Line::from(vec![
            spinner_span(m, m.pal.status_stream),
            Span::styled(
                "STREAMING ",
                Style::default()
                    .fg(m.pal.status_stream)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else if m.turn_active {
        Line::from(vec![
            spinner_span(m, m.pal.status_work),
            Span::styled(
                "WORKING ",
                Style::default()
                    .fg(m.pal.status_work)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " READY ",
            Style::default()
                .fg(m.pal.status_ready)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let mut bar_spans = state_span.spans;
    bar_spans.push(Span::styled(
        format!(
            "  {} model calls · {} tool calls · {} tools · theme {}",
            m.model_calls_total,
            m.tool_calls_total,
            m.status.tool_names.len(),
            m.pal.name
        ),
        m.pal.fg(m.pal.info),
    ));
    f.render_widget(Paragraph::new(Line::from(bar_spans)), rows[3]);
}

/// Group tool names into display families, in stable registry order.
fn grouped_tools(names: &[String]) -> Vec<(&'static str, Vec<String>)> {
    let mut fs: Vec<String> = Vec::new();
    let mut shell: Vec<String> = Vec::new();
    let mut mem: Vec<String> = Vec::new();
    let mut skills: Vec<String> = Vec::new();
    let mut web: Vec<String> = Vec::new();
    let mut agent: Vec<String> = Vec::new();
    let mut meta: Vec<String> = Vec::new();
    let mut mcp: Vec<String> = Vec::new();
    let mut plugin: Vec<String> = Vec::new();
    for n in names {
        if n.starts_with("fs_") {
            fs.push(n.clone());
        } else if n.starts_with("shell_") {
            shell.push(n.clone());
        } else if n.starts_with("mem_") {
            mem.push(n.clone());
        } else if n.starts_with("skills_") {
            skills.push(n.clone());
        } else if n.starts_with("web_") {
            web.push(n.clone());
        } else if n.starts_with("agent_") {
            agent.push(n.clone());
        } else if n.starts_with("mcp_") {
            mcp.push(n.clone());
        } else if n.starts_with("plugin_") {
            plugin.push(n.clone());
        } else {
            meta.push(n.clone());
        }
    }
    let mut out: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (label, group) in [
        ("fs", fs),
        ("shell", shell),
        ("memory", mem),
        ("skills", skills),
        ("web", web),
        ("workforce", agent),
        ("mcp", mcp),
        ("plugins", plugin),
        ("meta", meta),
    ] {
        if !group.is_empty() {
            out.push((label, group));
        }
    }
    out
}
