//! Shared token-bucket speed limit.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Shared token-bucket rate limiter.
///
/// Unlike a naive per-chunk `sleep(bytes / limit)`, the bucket smooths bursts
/// across all connections: tokens accrue at the limit and each chunk waits
/// only for the deficit. Clone one `Arc<RateLimiter>` into every download
/// that should share the budget.
///
/// An optional live override ([`RateLimiter::with_global`]) changes the cap of
/// running downloads without restarting them (speed schedules). Its
/// convention: `u64::MAX` = no override active, `0` = unlimited.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    limit_bps: u64,
    global_bps: Option<Arc<AtomicU64>>,
    /// This download's own live cap (0 = none), set from its progress window.
    own_bps: Option<Arc<AtomicU64>>,
    state: Arc<Mutex<(f64, std::time::Instant)>>,
}

impl RateLimiter {
    /// `limit_bps` = bytes per second, 0 = unlimited.
    pub fn new(limit_bps: u64) -> Self {
        Self {
            limit_bps,
            global_bps: None,
            own_bps: None,
            state: Arc::new(Mutex::new((limit_bps as f64, std::time::Instant::now()))),
        }
    }

    pub fn with_global(mut self, global_bps: Option<Arc<AtomicU64>>) -> Self {
        self.global_bps = global_bps;
        self
    }

    /// A live cap for this download alone (bytes/s, 0 = none), on top of the
    /// others: the lowest one wins.
    pub fn with_own(mut self, own_bps: Option<Arc<AtomicU64>>) -> Self {
        self.own_bps = own_bps;
        self
    }

    /// The cap in force right now (bytes/s, 0 = unlimited).
    pub fn effective_limit(&self) -> u64 {
        let base = self.base_limit();
        match self.own_bps.as_ref().map(|o| o.load(Ordering::SeqCst)).filter(|v| *v > 0) {
            Some(own) if base == 0 => own,
            Some(own) => own.min(base),
            None => base,
        }
    }

    fn base_limit(&self) -> u64 {
        match &self.global_bps {
            Some(g) => {
                let v = g.load(Ordering::SeqCst);
                if v == u64::MAX {
                    self.limit_bps
                } else {
                    v.min(if self.limit_bps == 0 { v } else { self.limit_bps })
                }
            }
            None => self.limit_bps,
        }
    }

    /// Wait until `bytes` may be passed on.
    pub async fn consume(&self, bytes: u64) {
        let limit = self.effective_limit();
        if limit == 0 || bytes == 0 {
            return;
        }
        let wait_secs: f64 = {
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let (ref mut tokens, ref mut last) = *g;
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(*last).as_secs_f64();
            *last = now;
            *tokens = (*tokens + elapsed * limit as f64).min(limit as f64 * 2.0);
            *tokens -= bytes as f64;
            if *tokens < 0.0 {
                -(*tokens) / limit as f64
            } else {
                0.0
            }
        };
        if wait_secs <= 0.0 {
            return;
        }
        // Wait in short steps: a limit raised (or lifted) meanwhile takes
        // effect at once. At 1 KB/s one 64 KB chunk owed a minute's sleep,
        // and Unlimited only reached the download after it.
        let step = std::time::Duration::from_millis(250);
        let mut limit = limit;
        let mut left = std::time::Duration::from_secs_f64(wait_secs);
        while !left.is_zero() {
            let nap = left.min(step);
            tokio::time::sleep(nap).await;
            left -= nap;
            let now_limit = self.effective_limit();
            if now_limit == limit {
                continue;
            }
            let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let (ref mut tokens, ref mut last) = *g;
            if now_limit == 0 {
                // Unlimited: what was owed is forgiven.
                *tokens = 0.0;
                *last = std::time::Instant::now();
                return;
            }
            // What is still owed, paid at the new rate.
            let owed = left.as_secs_f64() * limit as f64;
            *tokens = tokens.max(-owed);
            left = std::time::Duration::from_secs_f64(owed / now_limit as f64);
            limit = now_limit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lifting_the_limit_ends_a_long_wait_at_once() {
        let global = Arc::new(AtomicU64::new(1024));
        let lim = Arc::new(RateLimiter::new(0).with_global(Some(global.clone())));
        // 64 KB at 1 KB/s owes about a minute.
        let l2 = lim.clone();
        let t = std::time::Instant::now();
        let wait = tokio::spawn(async move { l2.consume(64 * 1024).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        global.store(u64::MAX, Ordering::SeqCst);
        wait.await.unwrap();
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[tokio::test]
    async fn raising_the_limit_shortens_the_wait() {
        let global = Arc::new(AtomicU64::new(1024));
        let lim = Arc::new(RateLimiter::new(0).with_global(Some(global.clone())));
        let l2 = lim.clone();
        let t = std::time::Instant::now();
        let wait = tokio::spawn(async move { l2.consume(64 * 1024).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        global.store(1024 * 1024, Ordering::SeqCst);
        wait.await.unwrap();
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }
}
