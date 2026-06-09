//! Per-source-slug concurrency limiting for the Source Pool (CEX-S2-14,
//! `docs/improvement/agent.md:1959`).
//!
//! Multiple sub-agents can each hold tool handlers for the same source. Without
//! a bound they could fire concurrent calls at one MCP / REST backend and
//! overwhelm it ("打爆同一 MCP / REST 后端"). [`SourceThrottle`] caps the number
//! of in-flight calls *per source slug* with one shared semaphore per slug,
//! lazily created on first use.
//!
//! The throttle is shared (`Arc`-backed [`Clone`]): the [`SourcePool`](
//! super::SourcePool) owns one and threads a clone into every
//! [`SourceToolHandler`](super::SourceToolHandler) it builds, so all handlers
//! for the same slug — across sub-agents — contend on the *same* semaphore.
//!
//! A `limit` of `0` disables throttling (the back-compat default):
//! [`SourceThrottle::acquire`] returns `None` immediately and the call proceeds
//! unbounded.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Per-slug concurrency limiter for source tool calls.
#[derive(Clone)]
pub struct SourceThrottle {
    limit: usize,
    semaphores: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl Default for SourceThrottle {
    /// Disabled: no per-slug limit (back-compatible with the pre-throttle
    /// Source Pool).
    fn default() -> Self {
        Self::new(0)
    }
}

impl SourceThrottle {
    /// Build a throttle that allows at most `limit` concurrent calls per source
    /// slug. `limit == 0` disables throttling entirely.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            semaphores: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The configured per-slug concurrency limit (`0` == disabled).
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Acquire a permit for `slug`, awaiting if the slug is already at its
    /// limit. Returns `None` when throttling is disabled (`limit == 0`) — the
    /// caller then proceeds without a permit. The returned permit releases its
    /// slot on drop, so the caller holds it across the source call.
    pub async fn acquire(&self, slug: &str) -> Option<OwnedSemaphorePermit> {
        if self.limit == 0 {
            return None;
        }
        let semaphore = self.semaphore_for(slug);
        // INVARIANT: the per-slug semaphore is never closed (we never call
        // `close()`), so `acquire_owned` cannot return `AcquireError`.
        Some(
            semaphore
                .acquire_owned()
                .await
                .expect("source throttle semaphore is never closed"),
        )
    }

    /// Get-or-create the shared semaphore for `slug`. Recovers from a poisoned
    /// lock (the map stays valid; a poison only means a prior holder panicked).
    fn semaphore_for(&self, slug: &str) -> Arc<Semaphore> {
        let mut guard = self
            .semaphores
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .entry(slug.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(self.limit)))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    #[tokio::test]
    async fn disabled_throttle_yields_no_permit() {
        let throttle = SourceThrottle::default();
        assert_eq!(throttle.limit(), 0);
        assert!(
            throttle.acquire("any-slug").await.is_none(),
            "a disabled throttle must not hand out a permit",
        );
        // Explicit `new(0)` is equivalent to the default.
        assert!(SourceThrottle::new(0).acquire("any-slug").await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn limits_concurrency_per_slug() {
        let throttle = SourceThrottle::new(2);
        let current = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..16 {
            let throttle = throttle.clone();
            let current = Arc::clone(&current);
            let max_seen = Arc::clone(&max_seen);
            handles.push(tokio::spawn(async move {
                let _permit = throttle.acquire("backend").await;
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, Ordering::SeqCst);
                // Hold the permit briefly so contention is observable.
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                current.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for handle in handles {
            handle.await.expect("task must not panic");
        }

        assert_eq!(current.load(Ordering::SeqCst), 0, "all permits released");
        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "per-slug concurrency must never exceed the limit of 2, saw {}",
            max_seen.load(Ordering::SeqCst),
        );
    }

    #[tokio::test]
    async fn distinct_slugs_do_not_contend() {
        // With a limit of 1 per slug, two *different* slugs can still hold a
        // permit at the same time — the limit is per-slug, not global.
        let throttle = SourceThrottle::new(1);
        let permit_a = throttle.acquire("slug-a").await;
        let permit_b = throttle.acquire("slug-b").await;
        assert!(permit_a.is_some() && permit_b.is_some());
    }

    #[tokio::test]
    async fn permit_release_lets_a_waiter_proceed() {
        // A second acquire on a limit-1 slug must block until the first permit
        // drops, then proceed. Pin it with a short timeout so a regression that
        // broke release would fail (timeout) instead of hanging.
        let throttle = SourceThrottle::new(1);
        let first = throttle.acquire("solo").await.expect("first permit");

        let throttle_clone = throttle.clone();
        let waiter = tokio::spawn(async move { throttle_clone.acquire("solo").await.is_some() });

        // The waiter cannot finish while `first` is held.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "waiter must block while slot is held"
        );

        drop(first);
        let acquired = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter must proceed within 1s after release")
            .expect("waiter task must not panic");
        assert!(acquired, "waiter must acquire the freed permit");
    }
}
