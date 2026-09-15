//! Conversation ownership with cancellable background work and foreground priority.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
pub struct ConversationRuns {
    inner: Mutex<Owners>,
    changed: Notify,
}

#[derive(Default)]
struct Owners {
    active: HashMap<Uuid, Option<CancellationToken>>,
    waiting: HashSet<Uuid>,
}

pub struct ConversationLease {
    runs: Arc<ConversationRuns>,
    id: Uuid,
    pub cancellation: CancellationToken,
}

impl Drop for ConversationLease {
    fn drop(&mut self) {
        self.runs.inner.lock().unwrap().active.remove(&self.id);
        self.runs.changed.notify_waiters();
    }
}

struct ForegroundWaiter(Arc<ConversationRuns>, Uuid);

impl Drop for ForegroundWaiter {
    fn drop(&mut self) {
        self.0.inner.lock().unwrap().waiting.remove(&self.1);
        self.0.changed.notify_waiters();
    }
}

impl ConversationRuns {
    pub fn is_active(&self, id: Uuid) -> bool {
        let owners = self.inner.lock().unwrap();
        owners.active.contains_key(&id) || owners.waiting.contains(&id)
    }

    pub fn claim_background(self: &Arc<Self>, id: Uuid) -> Option<ConversationLease> {
        let mut owners = self.inner.lock().unwrap();
        if owners.active.contains_key(&id) || owners.waiting.contains(&id) {
            return None;
        }
        let cancellation = CancellationToken::new();
        owners.active.insert(id, Some(cancellation.clone()));
        Some(ConversationLease {
            runs: self.clone(),
            id,
            cancellation,
        })
    }

    /// Only background work can be preempted. Another foreground run stays busy.
    pub async fn claim_foreground(self: &Arc<Self>, id: Uuid) -> Option<ConversationLease> {
        {
            let mut owners = self.inner.lock().unwrap();
            if owners.waiting.contains(&id) || matches!(owners.active.get(&id), Some(None)) {
                return None;
            }
            owners.waiting.insert(id);
            if let Some(Some(token)) = owners.active.get(&id) {
                token.cancel();
            }
        }
        let _waiter = ForegroundWaiter(self.clone(), id);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut owners = self.inner.lock().unwrap();
                if !owners.active.contains_key(&id) {
                    owners.active.insert(id, None);
                    return Some(ConversationLease {
                        runs: self.clone(),
                        id,
                        cancellation: CancellationToken::new(),
                    });
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn foreground_preempts_background_and_reserves_the_handoff() {
        let runs = Arc::new(ConversationRuns::default());
        let id = Uuid::new_v4();
        let background = runs.claim_background(id).unwrap();
        let foreground = tokio::spawn({
            let runs = runs.clone();
            async move { runs.claim_foreground(id).await.unwrap() }
        });
        background.cancellation.cancelled().await;
        assert!(runs.claim_background(id).is_none());
        assert!(runs.claim_foreground(id).await.is_none());
        drop(background);
        let foreground = foreground.await.unwrap();
        assert!(runs.is_active(id));
        assert!(runs.claim_background(id).is_none());
        drop(foreground);
        assert!(runs.claim_background(id).is_some());
    }

    #[tokio::test]
    async fn abandoned_foreground_waiter_does_not_leave_a_reservation() {
        let runs = Arc::new(ConversationRuns::default());
        let id = Uuid::new_v4();
        let background = runs.claim_background(id).unwrap();
        let waiter = tokio::spawn({
            let runs = runs.clone();
            async move { runs.claim_foreground(id).await }
        });
        background.cancellation.cancelled().await;
        waiter.abort();
        let _ = waiter.await;
        drop(background);
        assert!(runs.claim_background(id).is_some());
    }
}
