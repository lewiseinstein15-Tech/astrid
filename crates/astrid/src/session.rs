//! The session: composition root for one running Astrid. Owns the turn loop,
//! the approval and delegation flows, and the slash commands.

use crate::eventlog::EventPump;
use astrid_core::{
    capability_reply, classify, new_id, now_iso, small_talk_reply, Event, HookRegistry, Intent,
    NarrationKind, Profile, TurnOutcome,
};
use astrid_memory::MemoryKernel;
use astrid_providers::{
    ChatMessage, ChatRequest, LadderResult, ProviderLadder, Role, ToolSpec,
};
use astrid_tools::{ApprovalSink, DelegateSink, ToolCtx, ToolOutput, ToolRegistry};
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

        let ladder = Arc::new(ProviderLadder::from_config(&cfg.providers));
        let provider_desc = ladder
            .rung_summary()
            .first()
            .cloned()
            .unwrap_or_else(|| "offline".to_string());

        let session_started = Event::SessionStarted {
            session_id: session_id.clone(),
            config_fingerprint: format!(
                "profile={} providers={}",
                cfg.permissions.as_str(),
                cfg.providers.ladder.join(",")
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
                self.emit(Event::MessageDelta {
                    turn_id: turn_id.clone(),
                    text: reply.clone(),
                })
                .await;
                Ok(self
                    .close_turn(&turn_id, TurnOutcome::CompletedFastPath, Some(reply), 0)
                    .await)
            }
            Intent::CapabilityQuestion => {
                let reply = self.capability_answer();
                self.emit(Event::MessageDelta {
                    turn_id: turn_id.clone(),
                    text: reply.clone(),
                })
                .await;
                Ok(self
                    .close_turn(&turn_id, TurnOutcome::CompletedFastPath, Some(reply), 0)
                    .await)
            }
            Intent::Task => self.agent_turn(input, &turn_id).await,
        }
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
                "commands: /tools, /skills, /memory, /providers, /hooks, /quit — \
                 anything else goes to the agent"
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
        let mut tools_used = false;
        let mut answer: Option<String> = None;
        let mut provider_seen: Option<String> = None;
        let mut ladder_slid = 0usize;
        let mut outcome = TurnOutcome::Unverified;

        let mut messages = vec![ChatMessage::user(input.to_string())];
        let tool_specs = self.tool_specs();

        for step in 0..max_steps {
            if self.cancel.load(Ordering::SeqCst) {
                outcome = TurnOutcome::Cancelled;
                break;
            }
            if step == 0 {
                self.emit(Event::NarrationLine {
                    turn_id: turn_id.to_string(),
                    kind: NarrationKind::Ack,
                    text: format!("taking the task ({max_steps} step budget)"),
                })
                .await;
            }

            let req = ChatRequest {
                system: self.system_prompt(input),
                messages: messages.clone(),
                tools: tool_specs.clone(),
                temperature: 0.2,
                max_tokens: Some(2048),
            };
            let LadderResult {
                response,
                provider_id,
                ladder_slid: slid,
                model_calls: calls,
                ..
            } = match self.ladder.chat(&req).await {
                Ok(r) => r,
                Err(e) => {
                    outcome = TurnOutcome::Failed(e.to_string());
                    break;
                }
            };
            model_calls += calls;
            if provider_seen.is_none() {
                provider_seen = Some(provider_id.clone());
                ladder_slid = slid;
            }

            if response.tool_calls.is_empty() {
                answer = response.content.clone();
                if let Some(a) = &answer {
                    self.emit(Event::MessageDelta {
                        turn_id: turn_id.to_string(),
                        text: a.clone(),
                    })
                    .await;
                }
                outcome = if tools_used {
                    TurnOutcome::Completed
                } else {
                    TurnOutcome::Unverified
                };
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
                        self.emit(Event::NarrationLine {
                            turn_id: turn_id.to_string(),
                            kind: NarrationKind::Decision,
                            text: format!("malformed arguments for {name}: {err} — skipping"),
                        })
                        .await;
                        messages.push(ChatMessage::tool_result(
                            call.id.clone(),
                            format!("ERROR: malformed arguments: {err}"),
                        ));
                        continue;
                    }
                };
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
                        tools_used = true;
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
