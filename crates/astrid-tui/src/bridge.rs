//! Approval bridge: the TUI answers approval requests raised by the session's
//! `ApprovalSink` implementation. Exactly one approval may be pending; a
//! second request while one is pending is denied by the sink (never queued
//! silently).

use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[derive(Default)]
pub struct ApprovalBridge {
    pending: Mutex<Option<oneshot::Sender<bool>>>,
}

impl ApprovalBridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a responder for a new approval. Returns false if another
    /// approval is already pending (caller must deny).
    pub fn register(&self, tx: oneshot::Sender<bool>) -> bool {
        let mut guard = match self.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            return false;
        }
        *guard = Some(tx);
        true
    }

    /// Answer the pending approval. Returns false if nothing was pending.
    pub fn respond(&self, approved: bool) -> bool {
        let mut guard = match self.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.take() {
            Some(tx) => {
                let _ = tx.send(approved);
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
        assert!(bridge.register(tx));
        // second registration must fail
        let (tx2, _rx2) = oneshot::channel();
        assert!(!bridge.register(tx2));
        assert!(bridge.respond(true));
        assert_eq!(rx.await.unwrap(), true);
        assert!(!bridge.is_pending());
    }
}
