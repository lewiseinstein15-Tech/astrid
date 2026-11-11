//! The session: composition root for one running Astrid. Owns the turn loop,
//! the approval and delegation flows, and the slash commands.

use crate::eventlog::EventPump;
use astrid_core::{
    capability_reply, classify, new_id, now_iso, small_talk_reply, Event, HookRegistry, Intent,
    NarrationKind, Profile, TurnOutcome,
};
use astrid_mcp::McpClient;
use astrid_memory::MemoryKernel;
use astrid_providers::{
    ChatMessage, ChatRequest, LadderStream, ProviderLadder, Role, StreamItem, ToolSpec,
};
use astrid_tools::{ApprovalSink, DelegateSink, PluginTool, Tool, ToolCtx, ToolOutput, ToolRegistry};
use astrid_tui::ApprovalBridge;
use astrid_workforce::{Roster, SkillIndex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, Weak};
use tokio::sync::{broadcast, mpsc, oneshot};

pub struct TurnReport {
    pub turn_id: String,
    pub outcome: TurnOutcome,
    pub answer: Option<String>,
    pub provider: String,
    pub ladder_slid: usize,
    pub model_calls: usize,
}

/// Headless approval policy (TUI mode uses the live bridge instead).
#[derive(Clone, Copy)]
pub enum HeadlessApproval {
    Deny,
    Approve,
}

pub struct Session {
    pub cfg: astrid_core::AstridConfig,
    pub session_id: String,
    pub memory: Arc<MemoryKernel>,
    pub skills: Arc<SkillIndex>,
    pub roster: Roster,
    pub registry: Arc<ToolRegistry>,
    pub hooks: Arc<HookRegistry>,
    pub ladder: Arc<ProviderLadder>,
    pub provider_desc: String,
    /// Live MCP server clients (kept alive for the session; adapters hold Arcs).
    pub mcp_clients: Vec<Arc<McpClient>>,
    /// One line per MCP server: connection outcome, shown by /mcp and the TUI.
    pub mcp_report: Vec<String>,
    /// Plugin tool names loaded from `plugins/*/plugin.toml`.
    pub plugin_names: Vec<String>,
    /// Plugin manifests that failed to load — visible, never silent.
    pub plugin_problems: Vec<String>,
    /// Live approval bridge (TUI mode answers y/n; headless modes ignore it).
    pub approval_bridge: Arc<astrid_tui::ApprovalBridge>,
    events: mpsc::Sender<Event>,
    approval_flow: Arc<dyn ApprovalSink>,
    delegate: RwLock<Option<Arc<dyn DelegateSink>>>,
    pub cancel: Arc<AtomicBool>,
    confinement_root: std::path::PathBuf,
}

impl Session {
    /// Build and wire a session. Returns the session and a broadcast receiver
    /// that already carries `session.started`.
    pub async fn new(
        cfg: astrid_core::AstridConfig,
        confinement_root: std::path::PathBuf,
        headless: Option<HeadlessApproval>,
    ) -> astrid_core::Result<(Arc<Self>, broadcast::Receiver<Event>)> {
        let data_dir = cfg.effective_data_dir();
        std::fs::create_dir_all(&data_dir)?;
        let session_id = new_id("sess");

        let memory = Arc::new(MemoryKernel::open(&data_dir.join("memory"))?);
        let skills = Arc::new(SkillIndex::load(Some(&data_dir.join("skills")))?);
        let roster = Roster::bundled();
        let hooks = Arc::new(HookRegistry::load(&data_dir)?);

        let mut registry = Arc::new(ToolRegistry::with_builtins());
        // Safety: single-threaded construction before the registry is shared.
        let reg_mut = Arc::get_mut(&mut registry)
            .expect("registry not yet shared");
        reg_mut.gate = astrid_tools::PermissionGate { profile: cfg.permissions };

        // ---- plugins: manifest tools wired into the same registry ----
        let mut plugin_names = Vec::new();
        let mut plugin_problems = Vec::new();
        if cfg.plugins.enabled {
            let (plugin_tools, problems) = PluginTool::scan(&data_dir);
            for tool in plugin_tools {
                plugin_names.push(tool.name().to_string());
                reg_mut.register(Arc::new(tool));
            }
            plugin_problems = problems;
        }

        // ---- MCP servers: spawn, handshake, adapt tools into the registry ----
        let mut mcp_clients = Vec::new();
        let mut mcp_report = Vec::new();
        if cfg.mcp.enabled {
            for server_cfg in &cfg.mcp.servers {
                let name = &server_cfg.name;
                if !server_cfg.enabled {
                    mcp_report.push(format!("{name}: disabled in config"));
                    continue;
                }
                match astrid_mcp::connect_and_adapt(server_cfg, cfg.mcp.timeout_ms).await {
                    Ok((client, adapters)) => {
                        let count = adapters.len();
                        for adapter in adapters {
                            if reg_mut.get(adapter.name()).is_some() {
                                mcp_report.push(format!(
                                    "{name}: tool {} skipped (name collision)",
                                    adapter.name()
                                ));
                                continue;
                            }
                            reg_mut.register(Arc::new(adapter));
                        }
                        let info = client.server_info();
                        let server_name = info
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(name);
                        let version = info
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?");
                        mcp_report.push(format!(
                            "{name}: ✓ {server_name} v{version} · {count} tools"
                        ));
                        mcp_clients.push(client);
                    }
                    Err(e) => {
                        tracing::warn!(server = %name, error = %e, "mcp server failed to connect");
                        mcp_report.push(format!("{name}: ✗ {e}"));
                    }
                }
            }
        }

        let ladder = Arc::new(ProviderLadder::from_config(&cfg.providers));
        let provider_desc = ladder
            .rung_summary()
            .first()
            .cloned()
            .unwrap_or_else(|| "offline".to_string());

        let session_started = Event::SessionStarted {
            session_id: session_id.clone(),
            config_fingerprint: format!(
                "profile={} providers={} mcp={} plugins={}",
                cfg.permissions.as_str(),
                cfg.providers.ladder.join(","),
                mcp_clients.len(),
                plugin_names.len(),
            ),
        };
        let (pump, rx) = EventPump::start(data_dir.join("events.jsonl"), session_started);

        let bridge = astrid_tui::ApprovalBridge::new();
        let session = Arc::new(Self {
            cfg: cfg.clone(),
            session_id: session_id.clone(),
            memory: memory.clone(),
            skills: skills.clone(),
            roster,
            registry,
            hooks,
            ladder,
            provider_desc,
            mcp_clients,
            mcp_report,
            plugin_names,
            plugin_problems,
            approval_bridge: bridge.clone(),
            events: pump.tx.clone(),
            approval_flow: match headless {
                Some(HeadlessApproval::Deny) => Arc::new(DenyApproval) as Arc<dyn ApprovalSink>,
                Some(HeadlessApproval::Approve) => {
                    Arc::new(AlwaysApprove) as Arc<dyn ApprovalSink>
                }
                None => {
                    Arc::new(BridgeApproval { bridge: bridge.clone() }) as Arc<dyn ApprovalSink>
                }
            },
            delegate: RwLock::new(None),
            cancel: Arc::new(AtomicBool::new(false)),
            confinement_root,
        });

        let flow = Arc::new(DelegateFlow {
            session: Arc::downgrade(&session),
        });
        *session.delegate.write().expect("delegate lock") = Some(flow as Arc<dyn DelegateSink>);

        Ok((session, rx))
    }

    pub async fn emit(&self, ev: Event) {
        let _ = self.events.send(ev).await;
    }

    /// End the session: emits `session.ended`; the pump stops once all senders
    /// drop.
    pub async fn end(&self) {
        self.emit(Event::SessionEnded {
            session_id: self.session_id.clone(),
        })
        .await;
    }

    /// Run one user input through the turn pipeline.
    pub async fn turn(&self, input: &str) -> astrid_core::Result<TurnReport> {
        let turn_id = new_id("turn");
        let preview: String = input.chars().take(120).collect();
        self.emit(Event::TurnStarted {
            turn_id: turn_id.clone(),
            input_preview: preview.clone(),
        })
        .await;
        self.cancel.store(false, Ordering::SeqCst);

        if let Some(reply) = self.slash_command(input, &turn_id).await {
            return Ok(self
                .close_turn(&turn_id, TurnOutcome::CompletedFastPath, Some(reply), 0)
                .await);
        }

        match classify(input) {
            Intent::SmallTalk => {
                let reply = small_talk_reply(input);
                self.stream_paced(&turn_id, &reply).await;
                Ok(self
                    .close_turn(&turn_id, TurnOutcome::CompletedFastPath, Some(reply), 0)
                    .await)
            }
            Intent::CapabilityQuestion => {
                let reply = self.capability_answer();
                self.stream_paced(&turn_id, &reply).await;
                Ok(self
                    .close_turn(&turn_id, TurnOutcome::CompletedFastPath, Some(reply), 0)
                    .await)
            }
            Intent::Task => self.agent_turn(input, &turn_id).await,
        }
    }

    /// Word-boundary paced streaming: emit the text as MessageDelta chunks so
    /// the TUI visibly types it out. Bounded: a long answer never stalls the
    /// turn loop (cap on chunk count), and cancel stops the reveal.
    async fn stream_paced(&self, turn_id: &str, text: &str) {
        const CHUNK_MIN_CHARS: usize = 18;
        const PACE_MS: u64 = 14;
        const MAX_CHUNKS: usize = 150;
        let mut buf = String::new();
        let mut chunks = 0usize;
        for ch in text.chars() {
            buf.push(ch);
            let long_enough = buf.chars().count() >= CHUNK_MIN_CHARS;
            if long_enough && (ch == ' ' || ch == '\n') {
                self.emit(Event::MessageDelta {
                    turn_id: turn_id.to_string(),
                    text: buf.clone(),
                })
                .await;
                buf.clear();
                chunks += 1;
                if chunks >= MAX_CHUNKS {
                    continue; // drain the rest without pacing
                }
                tokio::time::sleep(std::time::Duration::from_millis(PACE_MS)).await;
                if self.cancel.load(Ordering::SeqCst) {
                    return;
                }
            }
        }
        if !buf.is_empty() {
            self.emit(Event::MessageDelta {
                turn_id: turn_id.to_string(),
                text: buf,
            })
            .await;
        }
    }

    async fn narrate(&self, turn_id: &str, kind: NarrationKind, text: impl Into<String>) {
        self.emit(Event::NarrationLine {
            turn_id: turn_id.to_string(),
            kind,
            text: text.into(),
        })
        .await;
    }

    async fn close_turn(
        &self,
        turn_id: &str,
        outcome: TurnOutcome,
        answer: Option<String>,
        model_calls: usize,
    ) -> TurnReport {
        let provider = self.provider_desc.clone();
        self.emit(Event::TurnCompleted {
            turn_id: turn_id.to_string(),
            outcome: outcome.clone(),
            provider: provider.clone(),
            ladder_slid: 0,
            model_calls,
        })
        .await;
        TurnReport {
            turn_id: turn_id.to_string(),
            outcome,
            answer,
            provider,
            ladder_slid: 0,
            model_calls,
        }
    }

    fn capability_answer(&self) -> String {
        let tools = self.registry.names();
        let skills: Vec<String> = self.skills.list().into_iter().map(|s| s.name).collect();
        capability_reply(
            &tools,
            &skills,
            self.roster.agents().len(),
            &self.provider_desc,
            self.memory.page_count(),
        )
    }

    // ---- slash commands -------------------------------------------------

    async fn slash_command(&self, input: &str, turn_id: &str) -> Option<String> {
        let cmd = input.trim();
        let text = match cmd {
            "/help" => Some(
                "commands: /tools, /skills, /memory, /providers, /mcp, /plugins, /hooks, \
                 /theme <name>, /clear (tui), /quit — anything else goes to the agent"
                    .to_string(),
            ),
            "/tools" => Some(format!("tools: {}", self.registry.names().join(", "))),
            "/skills" => Some(format!(
                "skills: {}",
                self.skills
                    .list()
                    .into_iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            "/memory" => Some(format!(
                "memory: {} pages (five-verb protocol v1: recall, remember, entity, synthesize, forget)",
                self.memory.page_count()
            )),
            "/providers" => Some(format!(
                "ladder: {}",
                self.ladder.rung_summary().join(" | ")
            )),
            "/mcp" => Some(if self.mcp_report.is_empty() {
                "mcp: no servers configured ([mcp.servers] in .astrid/config.toml)".to_string()
            } else {
                format!(
                    "mcp: {} client(s) live\n{}",
                    self.mcp_clients.len(),
                    self.mcp_report.join("\n")
                )
            }),
            "/plugins" => {
                let mut out = if self.plugin_names.is_empty() {
                    "plugins: none loaded (plugins/<name>/plugin.toml)".to_string()
                } else {
                    format!("plugins: {}", self.plugin_names.join(", "))
                };
                for p in &self.plugin_problems {
                    out.push_str(&format!("\nplugin problem: {p}"));
                }
                Some(out)
            }
            "/hooks" => Some(if self.hooks.is_empty() {
                "hooks: none registered (.astrid/hooks.toml)".to_string()
            } else {
                format!("hooks: {} registered", self.hooks.specs.len())
            }),
            _ => None,
        };
        if let Some(t) = text {
            self.emit(Event::NarrationLine {
                turn_id: turn_id.to_string(),
                kind: NarrationKind::Finding,
                text: t.clone(),
            })
            .await;
            if cmd == "/quit" {
                self.end().await;
            }
            return Some(t);
        }
        None
    }

    // ---- the agent turn loop --------------------------------------------

    async fn agent_turn(&self, input: &str, turn_id: &str) -> astrid_core::Result<TurnReport> {
        let max_steps = self.cfg.max_turn_steps;
        let mut model_calls = 0usize;
        let mut tools_used = 0usize;
        let mut streamed_deltas = false;
        let mut answer: Option<String> = None;
        let mut provider_seen: Option<String> = None;
        let mut ladder_slid = 0usize;
        let mut outcome = TurnOutcome::Unverified;

        let mut messages = vec![ChatMessage::user(input.to_string())];
        let tool_specs = self.tool_specs();

        self.narrate(
            turn_id,
            NarrationKind::Ack,
            format!("taking the task ({max_steps}-step budget)"),
        )
        .await;

        for step in 0..max_steps {
            if self.cancel.load(Ordering::SeqCst) {
                outcome = TurnOutcome::Cancelled;
                break;
            }
            if step > 0 {
                let observations = messages.iter().filter(|m| m.role == Role::Tool).count();
                self.narrate(
                    turn_id,
                    NarrationKind::Progress,
                    format!(
                        "step {}/{max_steps} — reasoning over {observations} tool result(s)",
                        step + 1
                    ),
                )
                .await;
            }

            let req = ChatRequest {
                system: self.system_prompt(input),
                messages: messages.clone(),
                tools: tool_specs.clone(),
                temperature: 0.2,
                max_tokens: Some(2048),
            };
            let LadderStream {
                mut rx,
                provider_id,
                ladder_slid: slid,
                ..
            } = match self.ladder.chat_stream(&req).await {
                Ok(r) => r,
                Err(e) => {
                    self.narrate(
                        turn_id,
                        NarrationKind::Decision,
                        format!("turn failed: {e}"),
                    )
                    .await;
                    outcome = TurnOutcome::Failed(e.to_string());
                    break;
                }
            };
            model_calls += 1;
            if provider_seen.is_none() {
                provider_seen = Some(provider_id.clone());
                ladder_slid = slid;
                if slid > 0 {
                    self.narrate(
                        turn_id,
                        NarrationKind::Decision,
                        format!(
                            "provider {provider_id} serving after {slid} skipped rung(s)"
                        ),
                    )
                    .await;
                }
            }

            // consume the stream: deltas forward live, Final closes the call
            let response = loop {
                match rx.recv().await {
                    Some(StreamItem::Delta(t)) => {
                        streamed_deltas = true;
                        self.emit(Event::MessageDelta {
                            turn_id: turn_id.to_string(),
                            text: t,
                        })
                        .await;
                    }
                    Some(StreamItem::Thinking(t)) => {
                        self.emit(Event::ThinkingDelta {
                            turn_id: turn_id.to_string(),
                            text: t,
                        })
                        .await;
                    }
                    Some(StreamItem::Failed(e)) => {
                        let msg = format!("stream failed: {e}");
                        self.narrate(turn_id, NarrationKind::Decision, msg.clone())
                            .await;
                        outcome = TurnOutcome::Failed(msg);
                        break None;
                    }
                    Some(StreamItem::Final(resp)) => break Some(resp),
                    None => {
                        let msg = "stream closed without a final response".to_string();
                        self.narrate(turn_id, NarrationKind::Decision, msg.clone())
                            .await;
                        outcome = TurnOutcome::Failed(msg);
                        break None;
                    }
                }
            };
            let Some(response) = response else {
                break;
            };

            if response.tool_calls.is_empty() {
                answer = response.content.clone();
                if let Some(a) = &answer {
                    // Providers without SSE support (offline included) hand
                    // over the full text at once; pace it so the transcript
                    // still streams.
                    if !streamed_deltas {
                        self.stream_paced(turn_id, a).await;
                    }
                }
                outcome = if tools_used > 0 {
                    TurnOutcome::Completed
                } else {
                    TurnOutcome::Unverified
                };
                if answer.is_some() {
                    self.narrate(
                        turn_id,
                        NarrationKind::Completion,
                        format!(
                            "answered after {} model call(s), {tools_used} tool call(s)",
                            model_calls
                        ),
                    )
                    .await;
                }
                break;
            }

            // observe tool calls (alias-bridged), then continue the loop
            messages.push(ChatMessage {
                role: Role::Assistant,
                content: response.content.clone().unwrap_or_default(),
                tool_call_id: None,
                tool_calls: response.tool_calls.clone(),
            });
            for call in &response.tool_calls {
                let (name, parsed_args) = self.resolve_tool_call(&call.name, &call.arguments_json);
                let args_value = match parsed_args {
                    Ok(v) => v,
                    Err(err) => {
                        self.narrate(
                            turn_id,
                            NarrationKind::Decision,
                            format!("malformed arguments for {name}: {err} — skipping"),
                        )
                        .await;
                        messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!("ERROR: malformed arguments: {err}"),
                        ));
                        continue;
                    }
                };
                self.narrate(
                    turn_id,
                    NarrationKind::Decision,
                    format!(
                        "acting: {name}({})",
                        truncate(&args_value.to_string(), 120)
                    ),
                )
                .await;
                let ctx = self.turn_ctx(turn_id);
                match self
                    .registry
                    .dispatch(
                        &ctx,
                        &name,
                        args_value,
                        &self.hooks,
                        self.cfg.hooks.timeout_ms,
                    )
                    .await
                {
                    Ok(out) => {
                        tools_used += 1;
                        messages.push(ChatMessage::tool_result(call.id.clone(), out.content));
                    }
                    Err(e) => {
                        messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!("ERROR: {e}"),
                        ));
                    }
                }
                if self.cancel.load(Ordering::SeqCst) {
                    outcome = TurnOutcome::Cancelled;
                    break;
                }
            }
            if matches!(outcome, TurnOutcome::Cancelled) {
                self.narrate(turn_id, NarrationKind::Progress, "cancelled by user")
                    .await;
                break;
            }
        }

        if matches!(outcome, TurnOutcome::Unverified) && answer.is_none() && model_calls > 0 {
            outcome = TurnOutcome::Unverified;
        }

        let provider = provider_seen.unwrap_or_else(|| "none".to_string());
        self.emit(Event::TurnCompleted {
            turn_id: turn_id.to_string(),
            outcome: outcome.clone(),
            provider: provider.clone(),
            ladder_slid,
            model_calls,
        })
        .await;
        Ok(TurnReport {
            turn_id: turn_id.to_string(),
            outcome,
            answer,
            provider,
            ladder_slid,
            model_calls,
        })
    }

    /// Alias bridge (Jexi M19 lesson): common names map onto real tools, but
    /// only when the real tool is actually offered — recovery can never grow
    /// the menu.
    fn resolve_tool_call(
        &self,
        name: &str,
        args_json: &str,
    ) -> (String, astrid_core::Result<serde_json::Value>) {
        const ALIASES: &[(&str, &str)] = &[
            ("write_file", "fs_write"),
            ("code_write", "fs_write"),
            ("read_file", "fs_read"),
            ("list_files", "fs_ls"),
            ("search_files", "fs_grep"),
            ("bash", "shell_run"),
            ("term_execute", "shell_run"),
            ("terminal_send", "shell_run"),
            ("memory_store", "mem_remember"),
            ("mem_store", "mem_remember"),
            ("memory_recall", "mem_recall"),
            ("memory_forget", "mem_forget"),
            ("load_skill", "skills_load"),
            ("list_skills", "skills_list"),
            ("fetch_url", "web_fetch"),
            ("web_get", "web_fetch"),
        ];
        let names = self.registry.names();
        let resolved = ALIASES
            .iter()
            .find(|(from, _)| *from == name)
            .map(|(_, to)| *to)
            .filter(|to| names.iter().any(|n| n == to))
            .unwrap_or(name);
        (resolved.to_string(), serde_json::from_str(args_json).map_err(astrid_core::Error::Json))
    }

    fn tool_specs(&self) -> Vec<ToolSpec> {
        self.registry
            .definitions()
            .into_iter()
            .map(|d| ToolSpec {
                name: d.name,
                description: d.description,
                parameters_json_schema: d.parameters_json_schema,
            })
            .collect()
    }

    fn turn_ctx(&self, turn_id: &str) -> ToolCtx {
        ToolCtx {
            session_id: self.session_id.clone(),
            turn_id: turn_id.to_string(),
            data_dir: self.cfg.effective_data_dir(),
            confinement_root: self.confinement_root.clone(),
            memory: self.memory.clone(),
            skills: self.skills.clone(),
            events: self.events.clone(),
            approval: self.approval_flow.clone(),
            delegate: self
                .delegate
                .read()
                .expect("delegate lock")
                .clone()
                .expect("delegate flow set at construction"),
            provider_desc: self.provider_desc.clone(),
            agent_count: self.roster.agents().len(),
        }
    }

    fn system_prompt(&self, input: &str) -> String {
        let mut p = String::new();
        p.push_str(&format!(
            "You are {}, a TUI-native agent operating system. \
             Today is {date}. Working directory: {cwd}.\n",
            self.cfg.agent_name,
            date = now_iso(),
            cwd = self.confinement_root.display(),
        ));
        p.push_str("Behavior contract: use tools for anything that touches the machine; \
             never claim a tool ran unless you called it; if you cannot complete the \
             task, say exactly what stopped you.\n");
        p.push_str("Tools:\n");
        for d in self.registry.definitions() {
            p.push_str(&format!("- {} — {}\n", d.name, d.description));
        }
        let hits = match self.memory.recall(input, 5) {
            Ok(h) => h,
            Err(_) => vec![],
        };
        if !hits.is_empty() {
            p.push_str("\nRelevant memory (five-verb protocol):\n");
            for h in hits {
                p.push_str(&format!("- [{}] {}\n", h.id, truncate(&h.snippet, 200)));
            }
        }
        let skills: Vec<String> = self
            .skills
            .list()
            .into_iter()
            .map(|s| s.name)
            .collect();
        p.push_str(&format!(
            "\nSkills available via skills_load: {}\n",
            skills.join(", ")
        ));
        p
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

// ---- approval flows ------------------------------------------------------

struct DenyApproval;
#[async_trait::async_trait]
impl ApprovalSink for DenyApproval {
    async fn request(&self, _tool: &str, details: &str) -> bool {
        tracing::warn!("headless denial: {details}");
        false
    }
}

struct AlwaysApprove;
#[async_trait::async_trait]
impl ApprovalSink for AlwaysApprove {
    async fn request(&self, _tool: &str, _details: &str) -> bool {
        true
    }
}

struct BridgeApproval {
    bridge: Arc<ApprovalBridge>,
}
#[async_trait::async_trait]
impl ApprovalSink for BridgeApproval {
    async fn request(&self, _tool: &str, _details: &str) -> bool {
        let (tx, rx) = oneshot::channel();
        if !self.bridge.register(tx) {
            return false; // another approval pending — deny, never queue silently
        }
        // details were already emitted as approval.requested by the registry
        rx.await.unwrap_or(false)
    }
}

// ---- delegation ----------------------------------------------------------

struct DelegateFlow {
    session: Weak<Session>,
}

#[async_trait::async_trait]
impl DelegateSink for DelegateFlow {
    async fn delegate(&self, agent_id: &str, task: &str) -> astrid_core::Result<String> {
        let session = self
            .session
            .upgrade()
            .ok_or_else(|| astrid_core::Error::Other("session gone".into()))?;
        let agent = session.roster.find(agent_id).ok_or_else(|| {
            astrid_core::Error::Other(format!(
                "unknown agent: {agent_id} (roster: {})",
                session
                    .roster
                    .agents()
                    .iter()
                    .map(|a| a.id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let agent_prompt = agent.system_prompt.clone();
        let agent_name = agent.name.clone();

        let _ = session
            .events
            .send(Event::NarrationLine {
                turn_id: new_id("sub"),
                kind: NarrationKind::Ack,
                text: format!("delegating to {agent_name} ({agent_id})"),
            })
            .await;

        // Read-only sub-turn: restricted gate, deny-all approvals, no nested
        // delegation, 3-step budget.
        let mut sub_registry = ToolRegistry::with_builtins();
        sub_registry.gate = astrid_tools::PermissionGate {
            profile: Profile::ReadOnly,
        };
        let sub_registry = Arc::new(sub_registry);

        let mut messages = vec![ChatMessage::user(task.to_string())];
        let sub_ctx = ToolCtx {
            session_id: session.session_id.clone(),
            turn_id: new_id("sub"),
            data_dir: session.cfg.effective_data_dir(),
            confinement_root: session.confinement_root.clone(),
            memory: session.memory.clone(),
            skills: session.skills.clone(),
            events: session.events.clone(),
            approval: Arc::new(DenyApproval) as Arc<dyn ApprovalSink>,
            delegate: Arc::new(NoDelegate) as Arc<dyn DelegateSink>,
            provider_desc: session.provider_desc.clone(),
            agent_count: session.roster.agents().len(),
        };

        for _ in 0..3 {
            let req = ChatRequest {
                system: agent_prompt.clone(),
                messages: messages.clone(),
                tools: sub_registry
                    .definitions()
                    .into_iter()
                    .map(|d| ToolSpec {
                        name: d.name,
                        description: d.description,
                        parameters_json_schema: d.parameters_json_schema,
                    })
                    .collect(),
                temperature: 0.2,
                max_tokens: Some(1024),
            };
            let resp = session.ladder.chat(&req).await?;
            if resp.response.tool_calls.is_empty() {
                return Ok(resp.response.content.unwrap_or_else(|| {
                    "subagent produced no answer".to_string()
                }));
            }
            messages.push(ChatMessage {
                role: Role::Assistant,
                content: resp.response.content.unwrap_or_default(),
                tool_call_id: None,
                tool_calls: resp.response.tool_calls.clone(),
            });
            for call in &resp.response.tool_calls {
                let args: serde_json::Value = serde_json::from_str(&call.arguments_json)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                match sub_registry
                    .dispatch(
                        &sub_ctx,
                        &call.name,
                        args,
                        &session.hooks,
                        session.cfg.hooks.timeout_ms,
                    )
                    .await
                {
                    Ok(ToolOutput { content, .. }) => {
                        messages.push(ChatMessage::tool_result(call.id.clone(), content));
                    }
                    Err(e) => {
                        messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!("ERROR: {e}"),
                        ));
                    }
                }
            }
        }
        Ok("subagent hit its 3-step budget without a final answer".to_string())
    }
}

struct NoDelegate;
#[async_trait::async_trait]
impl DelegateSink for NoDelegate {
    async fn delegate(&self, _agent_id: &str, _task: &str) -> astrid_core::Result<String> {
        Err(astrid_core::Error::Other(
            "nested delegation is not allowed".into(),
        ))
    }
}
