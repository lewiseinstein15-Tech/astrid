//! astrid-core — the vocabulary every Astrid crate speaks.
//!
//! Nothing in this crate depends on another Astrid crate. It defines events,
//! receipts, config, the kernel intent gate, and the hook contract.

pub mod config;
pub mod error;
pub mod events;
pub mod hooks;
pub mod ids;
pub mod kernel;

pub use config::{
    AstridConfig, BUILTIN_LADDER, HooksConfig, McpConfig, McpServerConfig, PluginsConfig, Profile,
    ProviderConfig, ProvidersConfig, TuiConfig,
};
pub use error::{Error, Result};
pub use events::{
    redact_args, Event, NarrationKind, Receipt, ReceiptStatus, Risk, TurnOutcome,
};
pub use hooks::{run_hook, HookDecision, HookEvent, HookRegistry, HookSpec, HOOK_EVENT_COUNT};
pub use ids::{new_id, now_iso};
pub use kernel::{capability_reply, classify, identity_reply, small_talk_reply, Intent};

/// Protocol version of the receipt/event taxonomy (see docs/PROTOCOLS.md).
pub const EVENT_TAXONOMY_VERSION: &str = "1.0";
