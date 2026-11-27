//! Typed configuration. Every key in `config.toml` is parsed into this struct;
//! unknown keys are reported as warnings at startup, never silently ignored.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Permission profile (docs/PROTOCOLS.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Safe tools only.
    ReadOnly,
    /// Safe + medium run; high and external ask for approval.
    #[default]
    Auto,
    /// Everything runs; developer profile.
    Full,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Profile::ReadOnly => "readonly",
            Profile::Auto => "auto",
            Profile::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// OpenAI-compatible base URL, e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: Option<String>,
    /// Model id as the provider names it.
    pub model: Option<String>,
    /// Environment-variable NAME holding the key. Never the key itself.
    pub key_ref: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            model: None,
            key_ref: None,
        }
    }
}

/// Built-in keyless rung ids, in default try-order. Canonically defined here
/// (the config crate cannot depend on providers); astrid-providers::builtin
/// mirrors this list and its tests assert the two never drift apart.
pub const BUILTIN_LADDER: [&str; 2] = ["kilo-auto/free", "pollinations"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Provider ids in try-order. The offline provider is implicit and last.
    pub ladder: Vec<String>,
    /// When true, only loopback endpoints are allowed (hermetic mode).
    pub local_only: bool,
    /// Per-provider settings keyed by provider id.
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl Default for ProvidersConfig {
    /// A fresh install gets the two built-in keyless rungs first (proven
    /// endpoints — see astrid-providers/src/builtin.rs), the user's own
    /// providers after, and the offline agent is implicit and last.
    fn default() -> Self {
        Self {
            ladder: BUILTIN_LADDER
                .iter()
                .map(|id| id.to_string())
                .collect(),
            local_only: false,
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    pub enabled: bool,
    /// Default per-hook timeout.
    pub timeout_ms: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 10_000,
        }
    }
}

/// TUI appearance. Themes live in `astrid-tui/src/theme.rs`; `/theme` switches
/// live (TUI-side), the config value is the startup default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// One of: `midnight` (default), `nordic`, `ember`.
    pub theme: String,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "midnight".to_string(),
        }
    }
}

/// One stdio MCP server to launch and wire at startup. The server speaks
/// JSON-RPC 2.0 over stdin/stdout (MCP `stdio` transport).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// Unique short id, used in tool names (`mcp_{server}_{tool}`).
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Argument vector for the executable.
    pub args: Vec<String>,
    /// Extra environment for the server process.
    pub env: BTreeMap<String, String>,
    /// Skip this server without spawning it.
    pub enabled: bool,
    /// Risk class override for this server's tools
    /// (`safe|medium|high|external`). Default `high`: external code is
    /// untrusted (MCP spec: clients SHOULD confirm sensitive operations).
    pub default_risk: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
            default_risk: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub enabled: bool,
    pub servers: Vec<McpServerConfig>,
    /// Per-request timeout for tools/list and tools/call.
    pub timeout_ms: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: Vec::new(),
            timeout_ms: 15_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginsConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AstridConfig {
    /// Where Astrid keeps its state. Default `./.astrid`.
    pub data_dir: PathBuf,
    pub permissions: Profile,
    /// Hard bound on think→act steps per turn.
    pub max_turn_steps: usize,
    /// Prefix used in narration lines.
    pub agent_name: String,
    pub providers: ProvidersConfig,
    pub hooks: HooksConfig,
    pub tui: TuiConfig,
    pub mcp: McpConfig,
    pub plugins: PluginsConfig,
}

impl Default for AstridConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(".astrid"),
            permissions: Profile::Auto,
            max_turn_steps: 8,
            agent_name: "Astrid".to_string(),
            providers: ProvidersConfig::default(),
            hooks: HooksConfig::default(),
            tui: TuiConfig::default(),
            mcp: McpConfig::default(),
            plugins: PluginsConfig::default(),
        }
    }
}

impl AstridConfig {
    /// Resolve the effective data dir: env `ASTRID_DATA_DIR` overrides config.
    pub fn effective_data_dir(&self) -> PathBuf {
        if let Ok(d) = std::env::var("ASTRID_DATA_DIR") {
            if !d.trim().is_empty() {
                return PathBuf::from(d);
            }
        }
        self.data_dir.clone()
    }

    /// Load config from `<data_dir>/config.toml`, creating a default file on
    /// first run. Returns warnings for unknown keys.
    pub fn load(data_dir: &Path) -> Result<(Self, Vec<String>)> {
        let path = data_dir.join("config.toml");
        if !path.exists() {
            let cfg = Self {
                data_dir: data_dir.to_path_buf(),
                ..Self::default()
            };
            let raw = toml::to_string_pretty(&cfg)
                .map_err(|e| Error::Config(format!("serialize default config: {e}")))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, raw)?;
            return Ok((cfg, vec![]));
        }
        let raw = std::fs::read_to_string(&path)?;
        let value: toml::Value = toml::from_str(&raw)?;
        let mut warnings = Vec::new();
        if let Some(table) = value.as_table() {
            let known = ["data_dir", "permissions", "max_turn_steps", "agent_name", "providers", "hooks", "tui", "mcp", "plugins"];
            for key in table.keys() {
                if !known.contains(&key.as_str()) {
                    warnings.push(format!("unknown config key `{key}` (ignored)"));
                }
            }
        }
        let mut cfg: Self = value.try_into()?;
        cfg.data_dir = data_dir.to_path_buf();
        Ok((cfg, warnings))
    }

    /// Write the config back to `<data_dir>/config.toml` (pretty). The MCP
    /// server-management surface (phase 6) uses this to persist server
    /// adds/removes/toggles made from the Android UI.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let path = data_dir.join("config.toml");
        let raw = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("serialize config: {e}")))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, raw)?;
        Ok(())
    }

    /// Validate `key_ref` fields: an env-var NAME or empty — never an inline key.
    pub fn validate(&self) -> Result<()> {
        for (id, p) in &self.providers.providers {
            if let Some(kr) = &p.key_ref {
                if kr.is_empty() {
                    continue;
                }
                let is_env_name = {
                    let mut cs = kr.chars();
                    let first_ok = cs.next().map(|c| c.is_ascii_uppercase() || c == '_').unwrap_or(false);
                    first_ok && cs.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                };
                let looks_inline = kr.starts_with("sk-")
                    || kr.starts_with("ghp_")
                    || kr.starts_with("AIza")
                    || kr.contains(' ');
                if !is_env_name || looks_inline {
                    return Err(Error::Config(format!(
                        "provider `{id}`: key_ref must be an ENV VAR NAME (A-Z, 0-9, _), never an inline key"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips() {
        let cfg = AstridConfig::default();
        let raw = toml::to_string_pretty(&cfg).unwrap();
        let back: AstridConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.agent_name, "Astrid");
        assert_eq!(back.permissions, Profile::Auto);
    }

    #[test]
    fn key_ref_validation_rejects_inline_keys() {
        let mut cfg = AstridConfig::default();
        cfg.providers.providers.insert(
            "groq".into(),
            ProviderConfig {
                key_ref: Some("sk-inline-key-123".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_err());

        cfg.providers.providers.get_mut("groq").unwrap().key_ref = Some("GROQ_API_KEY".into());
        assert!(cfg.validate().is_ok());
    }
}
