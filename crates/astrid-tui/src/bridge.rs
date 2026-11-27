//! Approval bridge: the TUI answers approval requests raised by the session's
//! `ApprovalSink` implementation. Exactly one approval may be pending; a
//! second request while one is pending is denied by the sink (never queued
//! silently).

use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// One waiting approval: the responder plus what the popup must show. The
/// kind + details ride ON the bridge (not on the audit stream) so the UI can
/// derive the popup while the turn loop holds the core mutex.
pub struct PendingApproval {
    tx: oneshot::Sender<bool>,
    kind: String,
    details: String,
}

#[derive(Default)]
pub struct ApprovalBridge {
    pending: Mutex<Option<PendingApproval>>,
}

impl ApprovalBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a responder for a new approval. Returns false if another
    /// approval is already pending (caller must deny).
    pub fn register(&self, kind: &str, details: &str, tx: oneshot::Sender<bool>) -> bool {
        let mut guard = match self.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(PendingApproval {
            tx,
            kind: kind.to_string(),
            details: details.to_string(),
        });
        true
    }

    /// The waiting request's (kind, details) for the popup — instant, no
    /// core mutex involved.
    pub fn pending_request(&self) -> Option<(String, String)> {
        let guard = match self.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .as_ref()
            .map(|p| (p.kind.clone(), p.details.clone()))
    }

    /// Answer the pending approval. Returns false if nothing was pending.
    pub fn respond(&self, approved: bool) -> bool {
        let mut guard = match self.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.take() {
            Some(p) => {
                let _ = p.tx.send(approved);
                true
            }
            None => false,
        }
    }

    pub fn is_pending(&self) -> bool {
        match self.pending.lock() {
            Ok(g) => g.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_pending_approval() {
        let bridge = ApprovalBridge::new();
        let (tx, rx) = oneshot::channel();
        assert!(bridge.register("turn", "{\"command\":\"rm -rf x\"}", tx));
        assert_eq!(
            bridge.pending_request(),
            Some(("turn".into(), "{\"command\":\"rm -rf x\"}".into()))
        );
        // second registration must fail
        let (tx2, _rx2) = oneshot::channel();
        assert!(!bridge.register("turn", "{}", tx2));
        assert!(bridge.respond(true));
        assert_eq!(rx.await.unwrap(), true);
        assert!(!bridge.is_pending());
    }
}
