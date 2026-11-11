//! The provider ladder (docs/PROTOCOLS.md §5): rungs walk in config order,
//! unconfigured rungs are honestly skipped, failures arm cooldowns, and the
//! offline agent always sits at the bottom so an answer is never a crash.
//! `ladder_slid` reports how many rungs were skipped before the winner — a
//! silent slide is a measurement bug, not a feature.

use crate::chat::{ChatRequest, Provider};
use crate::offline::OfflineProvider;
use crate::openai::OpenAICompatProvider;
use astrid_core::{Error, ProvidersConfig, Result};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Consecutive failures before a rung is sent to cooldown.
const COOLDOWN_AFTER_FAILURES: u32 = 3;
/// Default cooldown length when the provider sent no usable Retry-After.
const COOLDOWN_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct AttemptRecord {
    pub provider: String,
    pub skipped_reason: Option<String>,
    pub error: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
pub struct LadderResult {
    pub response: crate::chat::ChatResponse,
    pub provider_id: String,
    /// Rungs passed over before the winner — skipped (unconfigured /
    /// local-only / cooling down) or attempted-and-failed. A slide is always
    /// visible in the turn receipt; it is never silent.
    pub ladder_slid: usize,
    pub attempts: Vec<AttemptRecord>,
    pub model_calls: usize,
}

/// Per-rung rolling health: consecutive failures and the armed cooldown.
#[derive(Debug, Default)]
struct RungHealth {
    cooldown_until: Option<Instant>,
    consecutive_failures: u32,
}

struct Rung {
    id: String,
    /// `None` = unconfigured placeholder that is skipped without an attempt.
    provider: Option<Arc<dyn Provider>>,
    /// Why this rung is skipped without an attempt (unconfigured /
    /// local-only-blocked). Absent = the rung will actually serve.
    skip_reason: Option<String>,
}

pub struct ProviderLadder {
    rungs: Vec<Rung>,
    health: Mutex<HashMap<String, RungHealth>>,
}

/// key_ref → env var value. An unset or EMPTY env var yields no key, which is
/// valid: keyless lanes are first-class (PROTOCOLS.md §5).
fn resolve_key(key_ref: &Option<String>) -> Option<String> {
    match key_ref {
        Some(name) if !name.trim().is_empty() => std::env::var(name).ok().filter(|v| !v.is_empty()),
        _ => None,
    }
}

/// The OpenAI client surfaces `Retry-After` as `(retry-after Ns)` in the error
/// text; read it back here to arm an exactly-sized cooldown.
fn parse_retry_after(error: &str) -> Option<Duration> {
    let idx = error.find("retry-after ")?;
    let digits: String = error[idx + "retry-after ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok().map(Duration::from_secs)
}

impl ProviderLadder {
    /// Build rungs in config order; a rung is configured iff the providers map
    /// holds the id with BOTH base_url and model set. Unconfigured rungs stay
    /// as skipped placeholders so the ladder's shape is visible in receipts.
    /// The offline provider is appended last, always.
    pub fn from_config(cfg: &ProvidersConfig) -> Self {
        let mut rungs: Vec<Rung> = Vec::new();
        for id in &cfg.ladder {
            let pc = cfg.providers.get(id);
            let configured = pc.is_some_and(|p| p.base_url.is_some() && p.model.is_some());
            if !configured {
                let missing = match pc {
                    None => "no [providers] entry".to_string(),
                    Some(p) => match (&p.base_url, &p.model) {
                        (None, None) => "missing base_url and model".to_string(),
                        (None, Some(_)) => "missing base_url".to_string(),
                        (Some(_), None) => "missing model".to_string(),
                        _ => "unconfigured".to_string(),
                    },
                };
                rungs.push(Rung {
                    id: id.clone(),
                    provider: None,
                    skip_reason: Some(format!("unconfigured ({missing})")),
                });
                continue;
            }
            let p = pc.expect("checked above");
            let base_url = p.base_url.clone().expect("checked above");
            let model = p.model.clone().expect("checked above");
            let key = resolve_key(&p.key_ref);
            let provider: Arc<dyn Provider> =
                Arc::new(OpenAICompatProvider::new(id, &base_url, &model, key));
            let skip_reason = if cfg.local_only && !provider.is_local() {
                Some(format!(
                    "local_only=true excludes non-loopback endpoint {base_url}"
                ))
            } else {
                None
            };
            rungs.push(Rung {
                id: id.clone(),
                provider: Some(provider),
                skip_reason,
            });
        }
        rungs.push(Rung {
            id: "offline".to_string(),
            provider: Some(Arc::new(OfflineProvider)),
            skip_reason: None,
        });
        Self {
            rungs,
            health: Mutex::new(HashMap::new()),
        }
    }

    pub async fn chat(&self, req: &ChatRequest) -> Result<LadderResult> {
        let mut attempts: Vec<AttemptRecord> = Vec::new();
        let mut model_calls: usize = 0;
        let mut slid: usize = 0;
        let mut last_error: Option<String> = None;

        for rung in &self.rungs {
            let Some(provider) = &rung.provider else {
                attempts.push(AttemptRecord {
                    provider: rung.id.clone(),
                    skipped_reason: Some(
                        rung.skip_reason
                            .clone()
                            .unwrap_or_else(|| "unconfigured".into()),
                    ),
                    error: None,
                    latency_ms: 0,
                });
                slid += 1;
                continue;
            };
            if let Some(reason) = &rung.skip_reason {
                attempts.push(AttemptRecord {
                    provider: rung.id.clone(),
                    skipped_reason: Some(reason.clone()),
                    error: None,
                    latency_ms: 0,
                });
                slid += 1;
                continue;
            }

            // Cooldown check (guard scoped so it never spans an await).
            {
                let mut health = self.health.lock().await;
                let entry = health.entry(rung.id.clone()).or_default();
                if let Some(until) = entry.cooldown_until {
                    if Instant::now() < until {
                        let remaining = until
                            .checked_duration_since(Instant::now())
                            .map(|d| d.as_secs() + 1)
                            .unwrap_or(1);
                        let reason = format!(
                            "cooldown until +{remaining}s (armed after {} consecutive failures)",
                            entry.consecutive_failures
                        );
                        attempts.push(AttemptRecord {
                            provider: rung.id.clone(),
                            skipped_reason: Some(reason),
                            error: None,
                            latency_ms: 0,
                        });
                        slid += 1;
                        continue;
                    }
                }
            }

            // Real attempt — this rung serves or records an error.
            let started = Instant::now();
            model_calls += 1;
            match provider.chat(req).await {
                Ok(response) => {
                    let latency_ms = started.elapsed().as_millis() as u64;
                    attempts.push(AttemptRecord {
                        provider: rung.id.clone(),
                        skipped_reason: None,
                        error: None,
                        latency_ms,
                    });
                    let mut health = self.health.lock().await;
                    *health.entry(rung.id.clone()).or_default() = RungHealth::default();
                    return Ok(LadderResult {
                        response,
                        provider_id: rung.id.clone(),
                        ladder_slid: slid,
                        attempts,
                        model_calls,
                    });
                }
                Err(e) => {
                    let latency_ms = started.elapsed().as_millis() as u64;
                    let error = e.to_string();
                    attempts.push(AttemptRecord {
                        provider: rung.id.clone(),
                        skipped_reason: None,
                        error: Some(error.clone()),
                        latency_ms,
                    });
                    slid += 1; // a failed rung also passed the request down
                    tracing::warn!(provider = %rung.id, error = %error, "ladder rung failed; sliding");
                    let mut health = self.health.lock().await;
                    let entry = health.entry(rung.id.clone()).or_default();
                    entry.consecutive_failures += 1;
                    if entry.consecutive_failures >= COOLDOWN_AFTER_FAILURES {
                        let secs = parse_retry_after(&error)
                            .map(|d| d.as_secs())
                            .filter(|s| *s > 0)
                            .unwrap_or(COOLDOWN_SECS);
                        entry.cooldown_until = Some(Instant::now() + Duration::from_secs(secs));
                        tracing::warn!(provider = %rung.id, secs, "rung sent to cooldown");
                    }
                    last_error = Some(error);
                }
            }
        }

        // The offline rung cannot fail; reaching here means every rung failed.
        Err(Error::Provider(last_error.unwrap_or_else(|| {
            "all ladder rungs failed (including offline)".to_string()
        })))
    }

    /// One line per rung for `astrid doctor`:
    /// `id: configured | unconfigured | local-only-blocked`.
    pub fn rung_summary(&self) -> Vec<String> {
        self.rungs
            .iter()
            .map(|r| {
                let state = if r.provider.is_none() {
                    "unconfigured"
                } else if r.skip_reason.is_some() {
                    "local-only-blocked"
                } else {
                    "configured"
                };
                format!("{}: {}", r.id, state)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatMessage, ToolSpec};

    fn req() -> ChatRequest {
        ChatRequest {
            system: String::new(),
            messages: vec![ChatMessage::user("hi")],
            tools: Vec::<ToolSpec>::new(),
            temperature: 0.0,
            max_tokens: None,
        }
    }

    fn provider_cfg(base_url: &str) -> astrid_core::ProviderConfig {
        astrid_core::ProviderConfig {
            base_url: Some(base_url.to_string()),
            model: Some("test-model".to_string()),
            key_ref: None,
        }
    }

    #[tokio::test]
    async fn empty_config_offline_serves_with_zero_slide() {
        let ladder = ProviderLadder::from_config(&ProvidersConfig::default());
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.response.provider, "offline");
        assert_eq!(out.ladder_slid, 0);
        assert_eq!(out.model_calls, 1);
        assert_eq!(out.attempts.len(), 1);
        assert!(out.attempts[0].skipped_reason.is_none());
    }

    #[tokio::test]
    async fn local_only_excludes_non_loopback_endpoints() {
        let mut cfg = ProvidersConfig::default();
        cfg.local_only = true;
        cfg.ladder = vec!["remote".into()];
        cfg.providers
            .insert("remote".into(), provider_cfg("https://example.com/v1"));
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        // Offline serves; the remote rung was skipped without a model call.
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.ladder_slid, 1);
        assert_eq!(out.model_calls, 1);
        let skipped = &out.attempts[0];
        assert_eq!(skipped.provider, "remote");
        assert!(skipped
            .skipped_reason
            .as_deref()
            .unwrap()
            .contains("local_only"));
    }

    #[tokio::test]
    async fn local_only_keeps_loopback_rung() {
        // A loopback endpoint is local, so local_only must NOT exclude it; the
        // rung is attempted (and fails against the dead port) before sliding.
        let mut cfg = ProvidersConfig::default();
        cfg.local_only = true;
        cfg.ladder = vec!["ollama".into()];
        cfg.providers
            .insert("ollama".into(), provider_cfg("http://127.0.0.1:9/v1"));
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.ladder_slid, 1);
        assert!(out.attempts[0].error.is_some());
        assert_eq!(out.model_calls, 2);
    }

    #[tokio::test]
    async fn unconfigured_rung_is_skipped_and_not_counted() {
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["ghost".into()];
        // No entry in providers map at all.
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.ladder_slid, 1);
        assert_eq!(out.model_calls, 1);
        assert!(out.attempts[0]
            .skipped_reason
            .as_deref()
            .unwrap()
            .contains("unconfigured"));

        // Partially configured (model set, base_url missing) also skips.
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["half".into()];
        cfg.providers.insert(
            "half".into(),
            astrid_core::ProviderConfig {
                base_url: None,
                model: Some("m".into()),
                key_ref: None,
            },
        );
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        assert!(out.attempts[0]
            .skipped_reason
            .as_deref()
            .unwrap()
            .contains("base_url"));
    }

    #[tokio::test]
    async fn http_500_falls_through_to_offline() {
        let base = crate::test_support::spawn_mock(crate::test_support::ERR_500_BODY).await;
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["mock".into()];
        cfg.providers.insert("mock".into(), provider_cfg(&base));
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.ladder_slid, 1);
        assert_eq!(out.model_calls, 2);
        assert_eq!(out.attempts[0].provider, "mock");
        assert!(out.attempts[0]
            .error
            .as_deref()
            .unwrap()
            .contains("HTTP 500"));
        assert!(out.attempts[1].skipped_reason.is_none());
    }

    #[tokio::test]
    async fn http_200_rung_wins_without_sliding() {
        let base = crate::test_support::spawn_mock(crate::test_support::OK_BODY).await;
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["mock".into()];
        cfg.providers.insert("mock".into(), provider_cfg(&base));
        let ladder = ProviderLadder::from_config(&cfg);
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "mock");
        assert_eq!(out.ladder_slid, 0);
        assert_eq!(out.model_calls, 1);
        assert_eq!(out.response.content.as_deref(), Some("pong"));
    }

    #[tokio::test]
    async fn three_consecutive_failures_arm_cooldown() {
        let base = crate::test_support::spawn_mock(crate::test_support::ERR_500_BODY).await;
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["mock".into()];
        cfg.providers.insert("mock".into(), provider_cfg(&base));
        let ladder = ProviderLadder::from_config(&cfg);

        // Three calls: each fails over to offline; the third arms the cooldown.
        for _ in 0..3 {
            let out = ladder.chat(&req()).await.unwrap();
            assert_eq!(out.provider_id, "offline");
        }
        // Fourth call: the mock rung is skipped for cooldown, not attempted.
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.provider_id, "offline");
        assert_eq!(out.model_calls, 1);
        assert_eq!(out.attempts.len(), 2);
        let skipped = &out.attempts[0];
        assert!(skipped
            .skipped_reason
            .as_deref()
            .unwrap()
            .starts_with("cooldown until"));
        assert_eq!(out.ladder_slid, 1);
    }

    #[tokio::test]
    async fn retry_after_arms_shorter_cooldown() {
        let base =
            crate::test_support::spawn_mock(crate::test_support::ERR_500_WITH_RETRY_AFTER).await;
        let mut cfg = ProvidersConfig::default();
        cfg.ladder = vec!["mock".into()];
        cfg.providers.insert("mock".into(), provider_cfg(&base));
        let ladder = ProviderLadder::from_config(&cfg);

        for _ in 0..3 {
            ladder.chat(&req()).await.unwrap();
        }
        // Retry-After 7s (< the 30s default) expires quickly: after it lapses
        // the rung is attempted again rather than skipped.
        tokio::time::sleep(Duration::from_millis(7500)).await;
        let out = ladder.chat(&req()).await.unwrap();
        assert_eq!(out.model_calls, 2, "cooldown should have expired");
        assert!(out.attempts[0].error.is_some());
    }

    #[test]
    fn rung_summary_reports_states() {
        let ladder = ProviderLadder::from_config(&ProvidersConfig::default());
        assert_eq!(
            ladder.rung_summary(),
            vec!["offline: configured".to_string()]
        );

        let mut cfg = ProvidersConfig::default();
        cfg.local_only = true;
        cfg.ladder = vec!["ghost".into(), "remote".into()];
        cfg.providers
            .insert("remote".into(), provider_cfg("https://example.com/v1"));
        cfg.providers
            .insert("ghost".into(), astrid_core::ProviderConfig::default());
        let ladder = ProviderLadder::from_config(&cfg);
        assert_eq!(
            ladder.rung_summary(),
            vec![
                "ghost: unconfigured".to_string(),
                "remote: local-only-blocked".to_string(),
                "offline: configured".to_string(),
            ]
        );
    }

    #[test]
    fn key_resolution_treats_empty_env_as_keyless() {
        // Unknown env var name → no key (valid keyless lane).
        assert_eq!(resolve_key(&Some("ASTRID_NO_SUCH_VAR_XYZ".into())), None);
        assert_eq!(resolve_key(&None), None);
        assert_eq!(resolve_key(&Some(String::new())), None);
        std::env::set_var("ASTRID_TEST_KEY_PRESENT", "sk-abc");
        assert_eq!(
            resolve_key(&Some("ASTRID_TEST_KEY_PRESENT".into())),
            Some("sk-abc".into())
        );
        std::env::set_var("ASTRID_TEST_KEY_EMPTY", "");
        assert_eq!(resolve_key(&Some("ASTRID_TEST_KEY_EMPTY".into())), None);
    }

    #[test]
    fn parse_retry_after_reads_seconds() {
        assert_eq!(
            parse_retry_after("x: HTTP 429 boom (retry-after 12s)"),
            Some(Duration::from_secs(12))
        );
        assert_eq!(parse_retry_after("no marker here"), None);
        assert_eq!(parse_retry_after("(retry-after Ns)"), None);
    }
}
