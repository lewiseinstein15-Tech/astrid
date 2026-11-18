//! astrid-ffi — the bridge between Astrid core and the Android UI (Kotlin via
//! UniFFI). Phase 3 fix 3: the UI holds no demo data. Every screen calls into
//! this bridge, and this bridge calls the REAL core:
//!
//! - Chat runs the actual `astrid::session::Session::turn` pipeline (kernel
//!   intent gate → capability answer / offline agent / provider ladder) on an
//!   embedded tokio runtime. The transcript, sessions, attachments and
//!   receipts are persisted in SQLite in app-private storage.
//! - Files picked via SAF (or captured by the camera) are ingested by the
//!   core: copied into app storage, hashed, deduped by sha256 and linked to
//!   the message that carried them.
//! - Memory, agents, skills, plugins, approval rules, scheduler rows,
//!   connector state and provider config all live here.
//!
//! The UI layer draws; every decision and every write lives on this side.

mod store;

use astrid::session::{HeadlessApproval, Session, TurnReport};
use astrid_core::config::{AstridConfig, Profile, ProviderConfig, ProvidersConfig};
use astrid_core::events::{Event, NarrationKind, TurnOutcome};
use astrid_memory::protocol::PageSummary;
use sha2::{Digest, Sha256};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{Store, now_ms};
use uniffi::Record;

pub use store::{ChatMessageRecord, ChatSessionRecord, ConnectorRecord, EventRecord, PhotoRecord, RuleRecord, TaskRecord, UploadRecord};

/// Who the app is. The UI renders this on its first screen instead of
/// hardcoding strings; the version comes from the repo-root VERSION file.
#[derive(Debug, Clone, Record)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub tagline: String,
}

/// Why a bridge call failed. Kotlin renders these in the trace, never
/// silently swallows them.
#[derive(Debug, uniffi::Error)]
pub enum BridgeError {
    Store { msg: String },
    NoStore,
    NoCore,
    Io { msg: String },
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Store { msg } => write!(f, "store error: {msg}"),
            BridgeError::NoStore => write!(f, "no store attached to this bridge"),
            BridgeError::NoCore => write!(f, "no core session attached to this bridge"),
            BridgeError::Io { msg } => write!(f, "io error: {msg}"),
        }
    }
}

/// One bundled skill from the compiled-in catalog.
#[derive(Debug, Clone, Record)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
}

/// One curated agent from the compiled-in roster.
#[derive(Debug, Clone, Record)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub division: String,
    pub role: String,
    pub capabilities: Vec<String>,
}

/// One memory page (five-verb protocol store).
#[derive(Debug, Clone, Record)]
pub struct MemoryPageRecord {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub forgotten: bool,
    pub updated_at: String,
}

/// One plugin discovered by the manifest scan (possibly none, possibly broken).
#[derive(Debug, Clone, Record)]
pub struct PluginRecord {
    pub name: String,
    pub problem: Option<String>,
}

/// The configured provider rung (persisted; applied to the ladder on rebuild).
#[derive(Debug, Clone, Record)]
pub struct ProviderRecord {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub has_key: bool,
}

/// The assistant answer of one turn, with the honest provider label.
#[derive(Debug, Clone, Record)]
pub struct TurnReply {
    pub reply: String,
    pub provider: String,
    pub turn_id: String,
    /// Why the answer came from a non-first ladder rung (rung errors and
    /// skips, verbatim); null when the first rung served. Rendered as-is:
    /// the fallback is shown, never hidden.
    pub fallback_reason: Option<String>,
}

/// One rung of the effective ladder, in try-order. The offline rung is
/// always present as the last row (it is implicit in the Rust ladder).
#[derive(Debug, Clone, Record)]
pub struct LadderRungRecord {
    pub id: String,
    /// One line of plain language: what this rung is.
    pub desc: String,
    /// True for the built-in keyless lanes (kilo-auto/free, pollinations).
    pub builtin: bool,
}

/// One check of the network self-test (Settings > Providers > Test).
#[derive(Debug, Clone, Record)]
pub struct NetSelfTestCheck {
    /// "dns" | "https" | "chat"
    pub name: String,
    pub ok: bool,
    /// Human-readable result: the status line, the first 200 chars of the
    /// body, or the RAW error chain — errors are surfaced, never swallowed.
    pub detail: String,
    /// Wall-clock milliseconds this check took.
    pub ms: u64,
}

/// The whole per-rung network self-test report.
#[derive(Debug, Clone, Record)]
pub struct NetSelfTestReport {
    pub rung: String,
    pub checks: Vec<NetSelfTestCheck>,
}

/// The embedded core: a tokio runtime plus the real session turn loop.
struct Core {
    rt: tokio::runtime::Runtime,
    session: Arc<Session>,
    data_dir: PathBuf,
}

/// Entry object handed to Kotlin. One instance, owned by the app process.
#[derive(uniffi::Object)]
pub struct Bridge {
    started_at_ms: u64,
    store: Option<Arc<Store>>,
    core: Mutex<Option<Core>>,
}

/// Map a core event to an audit-log (kind, text) row. Deltas are skipped:
/// they are rendering detail, not receipts.
fn event_row(event: &Event) -> Option<(String, String)> {
    match event {
        Event::SessionStarted { session_id, .. } => {
            Some(("session.started".into(), session_id.clone()))
        }
        Event::SessionEnded { session_id } => {
            Some(("session.ended".into(), session_id.clone()))
        }
        Event::TurnStarted { turn_id, input_preview } => Some((
            "turn.started".into(),
            format!("{turn_id}: {input_preview}"),
        )),
        Event::MessageDelta { .. } | Event::ThinkingDelta { .. } => None,
        Event::NarrationLine { kind, text, .. } => {
            let kind_tag = match kind {
                NarrationKind::Ack => "ack",
                NarrationKind::Finding => "finding",
                NarrationKind::Progress => "progress",
                NarrationKind::Decision => "decision",
                NarrationKind::Completion => "completion",
            };
            Some((format!("narration.{kind_tag}"), text.clone()))
        }
        Event::ToolStarted { receipt } => Some((
            "tool.started".into(),
            format!("{} [{:?}] {}", receipt.tool, receipt.risk, receipt.args_json),
        )),
        Event::ToolCompleted { tool, result_preview, duration_ms, .. } => Some((
            "tool.completed".into(),
            format!("{tool}: {result_preview} ({duration_ms}ms)"),
        )),
        Event::ToolFailed { tool, error, duration_ms, .. } => Some((
            "tool.failed".into(),
            format!("{tool}: {error} ({duration_ms}ms)"),
        )),
        Event::ToolRefused { tool, reason, .. } => {
            Some(("tool.refused".into(), format!("{tool}: {reason}")))
        }
        Event::ApprovalRequested { tool, details, .. } => {
            Some(("approval.requested".into(), format!("{tool}: {details}")))
        }
        Event::ApprovalResolved { approved, .. } => Some((
            "approval.resolved".into(),
            if *approved { "approved" } else { "denied" }.into(),
        )),
        Event::HookFired { event, script, decision } => Some((
            "hook.fired".into(),
            format!("{event} → {script} ({decision})"),
        )),
        Event::TurnCompleted { turn_id, outcome, provider, .. } => Some((
            "turn.completed".into(),
            format!("{turn_id}: {} via {provider}", outcome_label(outcome)),
        )),
        _ => Some(("event".into(), format!("{event:?}"))),
    }
}

fn outcome_label(outcome: &TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::CompletedFastPath => "fast path",
        TurnOutcome::AwaitingApproval => "awaiting approval",
        TurnOutcome::Cancelled => "cancelled",
        TurnOutcome::Failed(_) => "failed",
        TurnOutcome::Unverified => "unverified",
    }
}

fn map_store<T>(r: Result<T, rusqlite::Error>) -> Result<T, BridgeError> {
    r.map_err(|e| BridgeError::Store { msg: e.to_string() })
}

fn map_io<T>(r: Result<T, std::io::Error>) -> Result<T, BridgeError> {
    r.map_err(|e| BridgeError::Io { msg: e.to_string() })
}

/// Sanitize a user-supplied file name for on-disk storage.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() { "file".into() } else { trimmed.to_string() }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

const PROVIDER_KV: &str = "provider.cfg";
const ACTIVE_SESSION_KV: &str = "chat.active_session";
const LADDER_KV: &str = "ladder.order";

/// The effective ladder order: the saved order (Settings > Providers reorder)
/// wins; otherwise the built-in keyless rungs first, then the user's rung.
/// The offline rung is stripped — from_config re-appends it, always last.
fn ladder_order(saved: Option<Vec<String>>, custom: Option<String>) -> Vec<String> {
    let mut ladder = match saved {
        Some(v) if !v.is_empty() => v,
        _ => astrid_providers::BUILTIN_IDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    ladder.retain(|id| id != "offline");
    if let Some(c) = custom {
        if !ladder.iter().any(|id| id == &c) {
            ladder.push(c);
        }
    }
    ladder
}

/// Full error chain of any error, joined with ": " — a reqwest TLS or DNS
/// failure is worthless as "request failed"; the chain names the real cause.
fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(err) = src {
        s.push_str(": ");
        s.push_str(&err.to_string());
        src = err.source();
    }
    s
}

/// Host (and port) of a base_url. Returns (host, port) with the scheme
/// default applied when the authority carries no explicit port.
fn host_of(base_url: &str) -> (String, u16) {
    let rest = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(base_url);
    let authority = rest.split('/').next().unwrap_or(rest);
    let default_port = if base_url.starts_with("https") { 443 } else { 80 };
    match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.trim_matches(['[', ']']).to_string(), p.parse().unwrap_or(default_port))
        }
        _ => (
            authority.trim_matches(['[', ']']).to_string(),
            default_port,
        ),
    }
}

/// The configured custom provider, if any (base_url non-empty). Internal
/// helper outside the uniffi surface — ProviderCfg is not a Record.
fn custom_provider_cfg(
    bridge: &Bridge,
) -> Result<Option<ProviderCfg>, BridgeError> {
    let s = bridge.store.as_ref().ok_or(BridgeError::NoStore)?;
    match map_store(s.kv_get(PROVIDER_KV))? {
        Some(json) => Ok(serde_json::from_str::<ProviderCfg>(&json)
            .ok()
            .filter(|c| !c.base_url.is_empty())),
        None => Ok(None),
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct ProviderCfg {
    name: String,
    base_url: String,
    api_key: String,
    model: String,
}

fn provider_record(cfg: &ProviderCfg) -> ProviderRecord {
    ProviderRecord {
        name: cfg.name.clone(),
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
        has_key: !cfg.api_key.is_empty(),
    }
}

#[uniffi::export]
impl Bridge {
    /// Store-less bridge (phase-1 surface; unit tests use it on the host JVM).
    #[uniffi::constructor]
    pub fn new() -> Self {
        Bridge {
            started_at_ms: now_ms(),
            store: None,
            core: Mutex::new(None),
        }
    }

    /// Bridge with the audit/event log open at `path`. Every open logs a
    /// `core.boot` event, so the log shows every process start.
    #[uniffi::constructor]
    pub fn with_store(path: String) -> Result<Self, BridgeError> {
        let store = Store::open(&path).map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        store
            .append("core.boot", &format!("Astrid core {} booted", env!("ASTRID_VERSION")))
            .map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        Ok(Bridge {
            started_at_ms: now_ms(),
            store: Some(Arc::new(store)),
            core: Mutex::new(None),
        })
    }

    /// Bridge with the full core attached: the same session turn loop the
    /// TUI runs, plus the SQLite store. `data_dir` is app-private storage;
    /// tools are confined to it; high-risk tool calls are denied headlessly
    /// (approvals enforcement arrives in phase 9).
    #[uniffi::constructor]
    pub fn with_core(db_path: String, data_dir: String) -> Result<Self, BridgeError> {
        let store = Store::open(&db_path).map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        store
            .append("core.boot", &format!("Astrid core {} booted", env!("ASTRID_VERSION")))
            .map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        let bridge = Bridge {
            started_at_ms: now_ms(),
            store: Some(Arc::new(store)),
            core: Mutex::new(None),
        };
        bridge.attach_core(PathBuf::from(data_dir))?;
        Ok(bridge)
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            name: "Astrid".into(),
            version: env!("ASTRID_VERSION").into(),
            tagline: "agent OS for Android".into(),
        }
    }

    pub fn greeting(&self) -> String {
        format!("Astrid core {} ready", env!("ASTRID_VERSION"))
    }

    pub fn uptime_ms(&self) -> u64 {
        now_ms().saturating_sub(self.started_at_ms)
    }

    pub fn permissions_profile(&self) -> String {
        self.with_core_ref(|core| core.session.cfg.permissions.as_str().to_string())
            .unwrap_or_else(|| "readonly (core not attached)".into())
    }

    // ---- event log ------------------------------------------------------

    pub fn append_event(&self, kind: String, text: String) -> Result<EventRecord, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.append(&kind, &text))
    }

    pub fn recent_events(&self, limit: u32) -> Result<Vec<EventRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.recent(limit))
    }

    pub fn event_count(&self) -> Result<u64, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.count())
    }

    // ---- chat -----------------------------------------------------------

    /// Start a new chat: a fresh persisted session row becomes the active one.
    pub fn new_chat(&self) -> Result<ChatSessionRecord, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let row = map_store(s.insert_session("New chat"))?;
        map_store(s.kv_set(ACTIVE_SESSION_KV, &row.id.to_string()))?;
        self.append_event("chat.new".into(), format!("session {}", row.id))?;
        Ok(row)
    }

    pub fn chat_sessions(&self) -> Result<Vec<ChatSessionRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_sessions())
    }

    /// Make a persisted session the active chat.
    pub fn open_session(&self, id: i64) -> Result<(), BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.kv_set(ACTIVE_SESSION_KV, &id.to_string()))?;
        self.append_event("chat.open".into(), format!("session {id}"))?;
        Ok(())
    }

    /// The active chat's transcript, oldest first. Creates a session lazily
    /// so a fresh install has a chat to type into.
    pub fn recent_messages(&self, limit: u32) -> Result<Vec<ChatMessageRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let session_id = self.active_session(s)?;
        map_store(s.recent_messages(session_id, limit))
    }

    /// Run a REAL turn through the core and persist both sides of the
    /// exchange. The reply is computed by the session pipeline (kernel intent
    /// gate → capability answer / offline agent / provider ladder), never by
    /// the UI.
    pub fn send_message(&self, text: String) -> Result<TurnReply, BridgeError> {
        self.send_message_with_files(text, Vec::new())
    }

    /// Send with attachments: every file is ingested (same sha256 = same
    /// upload, no duplicate rows), linked to the user message, and the turn
    /// runs as usual.
    pub fn send_message_with_files(
        &self,
        text: String,
        paths: Vec<String>,
    ) -> Result<TurnReply, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let session_id = self.active_session(s)?;
        let mut upload_ids = Vec::new();
        for path in &paths {
            let path_buf = PathBuf::from(path);
            let name = path_buf
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let rec = self.ingest_upload(name, path.clone())?;
            upload_ids.push(rec.id);
        }
        let user = map_store(s.insert_message(session_id, "user", &text, ""))?;
        map_store(s.link_message_files(user.id, &upload_ids))?;

        let (answer, provider, turn_id, fallback_reason) = self.core_call(|core| {
            let report: TurnReport = core
                .rt
                .block_on(core.session.turn(&text))
                .map_err(|e| BridgeError::Io { msg: e.to_string() })?;
            Ok((
                report.answer.unwrap_or_default(),
                report.provider,
                report.turn_id,
                report.fallback_reason,
            ))
        })??;
        map_store(s.insert_message(session_id, "assistant", &answer, &provider))?;
        Ok(TurnReply {
            reply: answer,
            provider,
            turn_id,
            fallback_reason,
        })
    }

    // ---- uploads (picked or captured files, copied + hashed by the core) --

    pub fn ingest_upload(&self, name: String, src_path: String) -> Result<UploadRecord, BridgeError> {
        let dir = self.core_call(|core| core.data_dir.join("uploads"))?;
        map_io(std::fs::create_dir_all(&dir))?;
        let dest = dir.join(format!("{}_{}", now_ms(), safe_name(&name)));
        map_io(std::fs::copy(&src_path, &dest))?;
        let bytes = map_io(std::fs::metadata(&dest))?.len();
        let content = map_io(std::fs::read(&dest))?;
        let rec = map_store(
            self.store
                .as_ref()
                .ok_or(BridgeError::NoStore)?
                .insert_upload(&safe_name(&name), &sha256_hex(&content), bytes),
        )?;
        self.append_event(
            "upload.ingested".into(),
            format!("{} ({} bytes, sha256 {}…)", rec.name, rec.bytes, &rec.sha256[..12.min(rec.sha256.len())]),
        )?;
        Ok(rec)
    }

    pub fn uploads(&self) -> Result<Vec<UploadRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_uploads())
    }

    // ---- photos (camera captures recorded by the bridge) ------------------

    pub fn record_photo(&self, path: String) -> Result<PhotoRecord, BridgeError> {
        let bytes = map_io(std::fs::metadata(&path))?.len();
        let rec = map_store(
            self.store
                .as_ref()
                .ok_or(BridgeError::NoStore)?
                .insert_photo(&path, bytes),
        )?;
        self.append_event("photo.saved".into(), format!("{} ({} bytes)", path.rsplit('/').next().unwrap_or(&path), rec.bytes))?;
        Ok(rec)
    }

    pub fn photos(&self) -> Result<Vec<PhotoRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_photos())
    }

    // ---- memory (five-verb protocol, markdown store) ----------------------

    pub fn remember(&self, topic: String, body: String) -> Result<String, BridgeError> {
        let id = self.core_call(|core| {
            core.session
                .memory
                .remember(&topic, &body, "note")
                .map_err(|e| BridgeError::Io { msg: e.to_string() })
        })??;
        self.append_event("memory.remember".into(), format!("{topic}: {}", body.chars().take(60).collect::<String>()))?;
        Ok(id)
    }

    pub fn memories(&self) -> Result<Vec<MemoryPageRecord>, BridgeError> {
        let pages: Vec<PageSummary> = self.core_call(|core| core.session.memory.list_pages())?;
        Ok(pages
            .into_iter()
            .map(|p| MemoryPageRecord {
                id: p.id,
                title: p.title,
                kind: p.kind,
                forgotten: p.forgotten,
                updated_at: p.updated_at,
            })
            .collect())
    }

    pub fn forget_memory(&self, id: String) -> Result<bool, BridgeError> {
        let ok = self.core_call(|core| {
            core.session
                .memory
                .forget(&id, "removed from the Android UI")
                .is_ok()
        })?;
        self.append_event("memory.forget".into(), id.clone())?;
        Ok(ok)
    }

    // ---- workforce (compiled-in roster + skills catalog) -------------------

    pub fn agents(&self) -> Result<Vec<AgentRecord>, BridgeError> {
        self.core_call(|core| {
            core.session
                .roster
                .agents()
                .iter()
                .map(|a| AgentRecord {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    division: a.division.clone(),
                    role: a.role.clone(),
                    capabilities: a.capabilities.clone(),
                })
                .collect()
        })
    }

    pub fn skills(&self) -> Result<Vec<SkillRecord>, BridgeError> {
        self.core_call(|core| {
            core.session
                .skills
                .list()
                .into_iter()
                .map(|s| SkillRecord {
                    name: s.name,
                    description: s.description,
                    when_to_use: s.when_to_use,
                })
                .collect()
        })
    }

    /// Plugins found by the manifest scan; entries with a `problem` are the
    /// manifests that failed to load — visible, never silent.
    pub fn plugins(&self) -> Result<Vec<PluginRecord>, BridgeError> {
        self.core_call(|core| {
            let mut out: Vec<PluginRecord> = core
                .session
                .plugin_names
                .iter()
                .map(|n| PluginRecord { name: n.clone(), problem: None })
                .collect();
            out.extend(
                core.session
                    .plugin_problems
                    .iter()
                    .map(|p| PluginRecord { name: p.clone(), problem: Some(p.clone()) }),
            );
            out
        })
    }

    /// The MCP connection report lines the session recorded at startup.
    pub fn mcp_report(&self) -> Result<Vec<String>, BridgeError> {
        self.core_call(|core| core.session.mcp_report.clone())
    }

    // ---- connectors (state persisted here; intents launched by Android) ----

    pub fn connectors(&self) -> Result<Vec<ConnectorRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_connectors())
    }

    pub fn set_connector(&self, id: String, state: String) -> Result<(), BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.set_connector(&id, &state))?;
        self.append_event("connector.state".into(), format!("{id} → {state}"))?;
        Ok(())
    }

    // ---- approval rules (persisted; enforcement lands in phase 9) ----------

    pub fn rules(&self) -> Result<Vec<RuleRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_rules())
    }

    pub fn add_rule(&self, pattern: String, decision: String) -> Result<RuleRecord, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        if decision != "allow" && decision != "deny" {
            return Err(BridgeError::Io { msg: "decision must be allow or deny".into() });
        }
        let rec = map_store(s.add_rule(&pattern, &decision))?;
        self.append_event("rule.add".into(), format!("{pattern} → {decision}"))?;
        Ok(rec)
    }

    pub fn delete_rule(&self, id: i64) -> Result<bool, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let ok = map_store(s.delete_rule(id))?;
        if ok {
            self.append_event("rule.delete".into(), id.to_string())?;
        }
        Ok(ok)
    }

    // ---- provider config (applied to the ladder on save) --------------------

    pub fn provider(&self) -> Result<ProviderRecord, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let cfg: ProviderCfg = match map_store(s.kv_get(PROVIDER_KV))? {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| BridgeError::Store { msg: e.to_string() })?,
            None => ProviderCfg::default(),
        };
        Ok(provider_record(&cfg))
    }

    /// Persist the provider rung and rebuild the session so the ladder
    /// actually serves from it. The API key is stored app-privately and
    /// injected as an environment variable; the config references the var
    /// NAME (keyRef discipline), never the key.
    pub fn set_provider(
        &self,
        name: String,
        base_url: String,
        api_key: String,
        model: String,
    ) -> Result<ProviderRecord, BridgeError> {
        if name.trim().is_empty() {
            return Err(BridgeError::Io { msg: "provider name must not be empty".into() });
        }
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let cfg = ProviderCfg { name: name.trim().to_string(), base_url: base_url.trim().to_string(), api_key, model: model.trim().to_string() };
        let json = serde_json::to_string(&cfg)
            .map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        map_store(s.kv_set(PROVIDER_KV, &json))?;
        let data_dir = self.core_call(|core| core.data_dir.clone())?;
        self.attach_core(data_dir)?;
        self.append_event(
            "provider.configured".into(),
            format!("{} → {} ({})", cfg.name, cfg.base_url, cfg.model),
        )?;
        Ok(provider_record(&cfg))
    }

    /// The effective ladder in try-order (Settings > Providers renders and
    /// reorders these rows; the offline rung is always shown last).
    pub fn ladder_rungs(&self) -> Result<Vec<LadderRungRecord>, BridgeError> {
        let custom = custom_provider_cfg(self)?;
        let saved: Option<Vec<String>> = {
            let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
            match map_store(s.kv_get(LADDER_KV))? {
                Some(json) => serde_json::from_str::<Vec<String>>(&json).ok(),
                None => None,
            }
        };
        let mut rungs: Vec<LadderRungRecord> = ladder_order(saved, custom.as_ref().map(|c| c.name.clone()))
            .into_iter()
            .map(|id| {
                let (desc, builtin) = match astrid_providers::builtin_rung(&id) {
                    Some(b) => (b.desc.to_string(), true),
                    None => match &custom {
                        Some(c) if c.name == id => (
                            format!(
                                "your provider — {} ({})",
                                c.base_url,
                                if c.model.is_empty() { "model unset".to_string() } else { c.model.clone() }
                            ),
                            false,
                        ),
                        _ => (format!("configured rung {id}"), false),
                    },
                };
                LadderRungRecord { id, desc, builtin }
            })
            .collect();
        rungs.push(LadderRungRecord {
            id: "offline".to_string(),
            desc: "Local fallback agent — answers without a model, never fails".to_string(),
            builtin: false,
        });
        Ok(rungs)
    }

    /// Persist the user's rung order (Settings > Providers). The offline rung
    /// is implicit and always last. Rebuilds the core so the ladder serves
    /// in the new order. The saved order KEEPS "offline" as a row (the UI
    /// lists it); the ladder builder strips it — so a hermetic order of just
    /// ["offline"] is valid and yields offline-only service.
    pub fn set_ladder(&self, order: Vec<String>) -> Result<Vec<LadderRungRecord>, BridgeError> {
        let cleaned: Vec<String> = order
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if cleaned.is_empty() {
            return Err(BridgeError::Io {
                msg: "ladder order must contain at least one rung".into(),
            });
        }
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        let json = serde_json::to_string(&cleaned)
            .map_err(|e| BridgeError::Store { msg: e.to_string() })?;
        map_store(s.kv_set(LADDER_KV, &json))?;
        let data_dir = self.core_call(|core| core.data_dir.clone())?;
        self.attach_core(data_dir)?;
        self.append_event("ladder.order".into(), cleaned.join(" → "))?;
        self.ladder_rungs()
    }

    /// Per-rung network self-test (Settings > Providers > Test): DNS, HTTPS
    /// handshake (GET {base}/models), and a REAL chat request ("Say hi in 3
    /// words") with latency and the first 200 chars of the answer. Every
    /// failure carries the RAW error chain — never swallowed, never pretty.
    pub fn net_selftest(&self, rung_id: String) -> Result<NetSelfTestReport, BridgeError> {
        let rung_id = rung_id.trim().to_string();
        let (base_url, chat_path, model) = match astrid_providers::builtin_rung(&rung_id) {
            Some(b) => (
                b.base_url.to_string(),
                b.chat_path.to_string(),
                b.model.to_string(),
            ),
            None if rung_id == "offline" => {
                return Ok(NetSelfTestReport {
                    rung: rung_id,
                    checks: vec![NetSelfTestCheck {
                        name: "local".to_string(),
                        ok: true,
                        detail: "offline agent — no network involved, always answers".to_string(),
                        ms: 0,
                    }],
                });
            }
            None => {
                let cfg = custom_provider_cfg(self)?.ok_or_else(|| BridgeError::Io {
                    msg: format!("rung {rung_id} is not a configured provider"),
                })?;
                (
                    cfg.base_url.clone(),
                    "/chat/completions".to_string(),
                    if cfg.model.is_empty() {
                        "(model unset)".to_string()
                    } else {
                        cfg.model.clone()
                    },
                )
            }
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| BridgeError::Io { msg: err_chain(&e) })?;
        self.core_call(|core| {
            core.rt.block_on(async {
                let mut checks: Vec<NetSelfTestCheck> = Vec::new();

                // DNS lookup of the host.
                let t0 = Instant::now();
                let (host, port) = host_of(&base_url);
                let ms = t0.elapsed().as_millis() as u64;
                match (host.as_str(), port).to_socket_addrs() {
                    Ok(addrs) => {
                        let count = addrs.count();
                        checks.push(NetSelfTestCheck {
                            name: "dns".into(),
                            ok: count > 0,
                            detail: format!("{host} resolved to {count} address(es)"),
                            ms,
                        });
                    }
                    Err(e) => checks.push(NetSelfTestCheck {
                        name: "dns".into(),
                        ok: false,
                        detail: err_chain(&e),
                        ms,
                    }),
                }

                // HTTPS handshake + models listing.
                let t0 = Instant::now();
                match client.get(format!("{base_url}/models")).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let ms = t0.elapsed().as_millis() as u64;
                        checks.push(NetSelfTestCheck {
                            name: "https".into(),
                            ok: status.is_success(),
                            detail: format!(
                                "GET {base_url}/models → HTTP {}",
                                status.as_u16()
                            ),
                            ms,
                        });
                    }
                    Err(e) => {
                        let ms = t0.elapsed().as_millis() as u64;
                        checks.push(NetSelfTestCheck {
                            name: "https".into(),
                            ok: false,
                            detail: err_chain(&e),
                            ms,
                        });
                    }
                }

                // A REAL chat request, timed, first 200 chars of the body.
                let t0 = Instant::now();
                let body = serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "Say hi in 3 words"}],
                    "max_tokens": 512,
                });
                match client
                    .post(format!("{base_url}{chat_path}"))
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        let ms = t0.elapsed().as_millis() as u64;
                        if status.is_success() {
                            let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
                            let content = parsed
                                .as_ref()
                                .and_then(|v| {
                                    v["choices"][0]["message"]["content"]
                                        .as_str()
                                        .map(str::to_string)
                                })
                                .filter(|s| !s.trim().is_empty())
                                .or_else(|| {
                                    // reasoning-only reply: show it, honestly labeled
                                    parsed.as_ref().and_then(|v| {
                                        v["choices"][0]["message"]["reasoning"]
                                            .as_str()
                                            .map(|s| {
                                                format!("(reasoning-only reply) {s}")
                                            })
                                    })
                                })
                                .unwrap_or_else(|| {
                                    format!(
                                        "(unparsable body) {}",
                                        text.chars().take(200).collect::<String>()
                                    )
                                });
                            let first200: String = content.chars().take(200).collect();
                            checks.push(NetSelfTestCheck {
                                name: "chat".into(),
                                ok: true,
                                detail: format!(
                                    "HTTP {} in {ms} ms — \"{first200}\"",
                                    status.as_u16()
                                ),
                                ms,
                            });
                        } else {
                            let first200: String = text.chars().take(200).collect();
                            checks.push(NetSelfTestCheck {
                                name: "chat".into(),
                                ok: false,
                                detail: format!("HTTP {} — {first200}", status.as_u16()),
                                ms,
                            });
                        }
                    }
                    Err(e) => {
                        let ms = t0.elapsed().as_millis() as u64;
                        checks.push(NetSelfTestCheck {
                            name: "chat".into(),
                            ok: false,
                            detail: err_chain(&e),
                            ms,
                        });
                    }
                }
                NetSelfTestReport { rung: rung_id.clone(), checks }
            })
        })
    }

    // ---- scheduler (rows here; Android WorkManager does the timing) ---------

    pub fn add_task(&self, name: String, every_minutes: u32) -> Result<TaskRecord, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        if every_minutes < 15 {
            return Err(BridgeError::Io { msg: "WorkManager floor is 15 minutes".into() });
        }
        let rec = map_store(s.add_task(&name, every_minutes))?;
        self.append_event("task.add".into(), format!("{} every {}m", rec.name, rec.every_minutes))?;
        Ok(rec)
    }

    pub fn set_task_enabled(&self, id: i64, enabled: bool) -> Result<(), BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.set_task_enabled(id, enabled))?;
        self.append_event("task.state".into(), format!("{id} → {}", if enabled { "on" } else { "off" }))?;
        Ok(())
    }

    pub fn tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
        let s = self.store.as_ref().ok_or(BridgeError::NoStore)?;
        map_store(s.list_tasks())
    }
}

impl Bridge {
    /// Run a closure with the embedded core. Replaces a MutexGuard return:
    /// the guard type is internal, the result crosses the FFI.
    fn core_call<R>(&self, f: impl FnOnce(&Core) -> R) -> Result<R, BridgeError> {
        let mut guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let core = guard.as_mut().ok_or(BridgeError::NoCore)?;
        Ok(f(core))
    }

    fn with_core_ref<R>(&self, f: impl FnOnce(&Core) -> R) -> Option<R> {
        let guard = self.core.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(f)
    }

    fn active_session(&self, s: &Store) -> Result<i64, BridgeError> {
        if let Some(id) = map_store(s.kv_get(ACTIVE_SESSION_KV))? {
            if let Ok(id) = id.parse::<i64>() {
                return Ok(id);
            }
        }
        let row = map_store(s.insert_session("New chat"))?;
        map_store(s.kv_set(ACTIVE_SESSION_KV, &row.id.to_string()))?;
        Ok(row.id)
    }

    /// (Re)build the embedded core from the persisted provider config.
    fn attach_core(&self, data_dir: PathBuf) -> Result<(), BridgeError> {
        map_io(std::fs::create_dir_all(&data_dir))?;

        let mut cfg = AstridConfig::default();
        cfg.data_dir = data_dir.clone();
        cfg.permissions = Profile::Auto;
        cfg.mcp.enabled = false; // stdio MCP servers are a phase-9 surface
        let mut custom_name: Option<String> = None;
        if let Some(json) = map_store(
            self.store
                .as_ref()
                .ok_or(BridgeError::NoStore)?
                .kv_get(PROVIDER_KV),
        )? {
            if let Ok(pcfg) = serde_json::from_str::<ProviderCfg>(&json) {
                if !pcfg.base_url.is_empty() {
                    if !pcfg.api_key.is_empty() {
                        // keyRef discipline: config holds the env-var NAME.
                        std::env::set_var("ASTRID_PROVIDER_KEY", &pcfg.api_key);
                    }
                    cfg.providers = ProvidersConfig {
                        ladder: Vec::new(), // the effective order is set below
                        local_only: false,
                        providers: [(
                            pcfg.name.clone(),
                            ProviderConfig {
                                base_url: Some(pcfg.base_url.clone()),
                                model: (!pcfg.model.is_empty()).then_some(pcfg.model.clone()),
                                key_ref: (!pcfg.api_key.is_empty())
                                    .then(|| "ASTRID_PROVIDER_KEY".to_string()),
                            },
                        )]
                        .into_iter()
                        .collect(),
                    };
                    custom_name = Some(pcfg.name.clone());
                }
            }
        }
        // Effective ladder (Phase 4.1): the saved order wins (Settings >
        // Providers reorder); otherwise built-in keyless rungs first, then
        // the user's own rung; the offline agent is implicit and last.
        let saved: Option<Vec<String>> = match map_store(
            self.store
                .as_ref()
                .ok_or(BridgeError::NoStore)?
                .kv_get(LADDER_KV),
        )? {
            Some(json) => serde_json::from_str::<Vec<String>>(&json).ok(),
            None => None,
        };
        cfg.providers.ladder = ladder_order(saved, custom_name);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| BridgeError::Io { msg: e.to_string() })?;

        let (session, mut events) = rt.block_on(Session::new(
            cfg,
            data_dir.clone(),
            Some(HeadlessApproval::Deny),
        ))
        .map_err(|e| BridgeError::Io { msg: e.to_string() })?;

        // Drain core events into the audit log for the Live trace screen.
        let store = self.store.clone().ok_or(BridgeError::NoStore)?;
        rt.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Some((kind, text)) = event_row(&event) {
                            let _ = store.append(&kind, &text);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });

        *self
            .core
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Core { rt, session, data_dir });
        Ok(())
    }
}

uniffi::setup_scaffolding!("astrid_ffi");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_order_handles_the_hermetic_offline_only_order() {
        // setLadder(["offline"]) — the UI/test hermetic switch — must yield
        // an empty pre-offline ladder (from_config re-appends offline).
        assert_eq!(
            ladder_order(Some(vec!["offline".to_string()]), None),
            Vec::<String>::new()
        );
        // Fresh install: built-ins first; the user's rung follows.
        assert_eq!(
            ladder_order(None, Some("mine".to_string())),
            vec!["kilo-auto/free".to_string(), "pollinations".to_string(), "mine".to_string()]
        );
    }

    #[test]
    fn app_info_matches_the_version_file() {
        let b = Bridge::new();
        let info = b.app_info();
        assert_eq!(info.name, "Astrid");
        assert_eq!(info.version, env!("ASTRID_VERSION"));
        assert!(!info.tagline.is_empty());
    }

    #[test]
    fn greeting_carries_the_version() {
        let b = Bridge::new();
        assert!(b.greeting().contains(env!("ASTRID_VERSION")));
    }

    #[test]
    fn uptime_is_small_and_monotonic() {
        let b = Bridge::new();
        let first = b.uptime_ms();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let second = b.uptime_ms();
        assert!(first <= 50, "fresh bridge should report ~0 uptime");
        assert!(second >= first, "uptime must be monotonic");
    }

    #[test]
    fn bridge_without_store_reports_nostore() {
        let b = Bridge::new();
        let err = b.append_event("x".into(), "y".into()).unwrap_err();
        assert!(matches!(err, BridgeError::NoStore));
    }

    #[test]
    fn with_store_logs_a_boot_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("astrid.db").to_string_lossy().into_owned();
        let b = Bridge::with_store(path.clone()).expect("bridge with store");
        let rows = b.recent_events(10).expect("recent");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "core.boot");
        assert!(rows[0].text.contains(env!("ASTRID_VERSION")));
        assert_eq!(b.event_count().expect("count"), 1);
    }

    /// The real chat path: a full core must answer "hi" from the kernel gate
    /// — the reply is computed by Rust, persisted in SQLite, and labeled
    /// with its provider.
    #[test]
    fn send_message_runs_the_real_turn_pipeline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("astrid.db").to_string_lossy().into_owned();
        let data = dir.path().join("data").to_string_lossy().into_owned();
        let b = Bridge::with_core(db, data).expect("bridge with core");

        let reply = b.send_message("hi".into()).expect("turn");
        assert!(
            reply.reply.contains("Astrid"),
            "small talk must be answered by the kernel gate, got: {}",
            reply.reply
        );
        assert_eq!(
            reply.provider, "local (no model)",
            "small talk is answered by the kernel gate, never by a provider"
        );

        let msgs = b.recent_messages(10).expect("messages");
        assert_eq!(msgs.len(), 2, "user + assistant persisted");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].text, "hi");
        assert_eq!(msgs[1].role, "assistant");

        // sessions listing shows the lazily created chat
        let sessions = b.chat_sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "hi");
        assert_eq!(sessions[0].message_count, 2);
    }

    #[test]
    fn send_with_files_ingests_links_and_dedupes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("astrid.db").to_string_lossy().into_owned();
        let data = dir.path().join("data").to_string_lossy().into_owned();
        let b = Bridge::with_core(db, data).expect("bridge with core");

        let src = dir.path().join("report.md");
        std::fs::write(&src, "attached content\n").unwrap();
        let p = src.to_string_lossy().into_owned();
        b.send_message_with_files("see attached".into(), vec![p.clone(), p.clone()])
            .expect("turn with files");

        // same sha256 ingested twice -> ONE upload row
        assert_eq!(b.uploads().expect("uploads").len(), 1);
        // the user bubble carries the file name
        let msgs = b.recent_messages(10).expect("messages");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].files, vec!["report.md".to_string()]);
        assert!(msgs[1].files.is_empty());
    }

    #[test]
    fn uploads_are_copied_hashed_and_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("astrid.db").to_string_lossy().into_owned();
        let data = dir.path().join("data").to_string_lossy().into_owned();
        let b = Bridge::with_core(db, data).expect("bridge with core");

        let src = dir.path().join("note.md");
        std::fs::write(&src, "# hello\nfrom a SAF pick\n").unwrap();
        let rec = b
            .ingest_upload("note.md".into(), src.to_string_lossy().into_owned())
            .expect("ingest");
        assert_eq!(rec.name, "note.md");
        assert!(rec.bytes > 0);
        assert_eq!(rec.sha256.len(), 64);
        assert_eq!(b.uploads().expect("list").len(), 1);
    }

    #[test]
    fn memory_rules_connectors_persist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("astrid.db").to_string_lossy().into_owned();
        let data = dir.path().join("data").to_string_lossy().into_owned();
        let b = Bridge::with_core(db, data).expect("bridge with core");

        b.remember("coffee".into(), "Oat latte, no sugar".into()).expect("remember");
        let pages = b.memories().expect("pages");
        assert!(pages.iter().any(|p| p.title.contains("coffee")));

        b.add_rule("rm -rf*".into(), "deny".into()).expect("rule");
        assert_eq!(b.rules().expect("rules").len(), 1);
        let rule = b.rules().unwrap()[0].clone();
        assert!(b.delete_rule(rule.id).expect("delete"));
        assert!(b.rules().expect("rules").is_empty());

        let connectors = b.connectors().expect("connectors");
        assert!(connectors.len() >= 3, "seeded connector rows");
        b.set_connector("github".into(), "connected".into()).expect("set");
        assert!(b
            .connectors()
            .unwrap()
            .iter()
            .any(|c| c.id == "github" && c.state == "connected"));

        let agents = b.agents().expect("agents");
        assert!(!agents.is_empty(), "bundled roster must be non-empty");
        let skills = b.skills().expect("skills");
        assert!(!skills.is_empty(), "bundled skills must be non-empty");
    }
}
