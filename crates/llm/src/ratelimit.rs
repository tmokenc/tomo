use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

/// Simple sliding-window rate limiter keyed by user id.
pub struct RateLimiter {
    window: Duration,
    capacity: usize,
    inner: DashMap<Id<UserMarker>, VecDeque<Instant>>,
}

impl RateLimiter {
    pub fn per_minute(capacity: u32) -> Self {
        Self {
            window: Duration::from_secs(60),
            capacity: capacity as usize,
            inner: DashMap::new(),
        }
    }

    /// Record a hit and return whether it was allowed.
    pub fn check(&self, user: Id<UserMarker>) -> bool {
        let now = Instant::now();
        let mut entry = self.inner.entry(user).or_default();
        while let Some(front) = entry.front() {
            if now.duration_since(*front) > self.window {
                entry.pop_front();
            } else {
                break;
            }
        }
        if entry.len() >= self.capacity {
            return false;
        }
        entry.push_back(now);
        true
    }
}
