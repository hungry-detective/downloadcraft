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
        if wait_secs > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs)).await;
        }
    }
}
