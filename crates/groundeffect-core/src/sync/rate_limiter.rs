//! Global rate limiter to avoid Google API throttling

use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing::debug;

/// Global rate limiter for all Google API requests
pub struct GlobalRateLimiter {
    limiter: Arc<
        RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
}

impl GlobalRateLimiter {
    /// Create a new rate limiter with the specified requests per second
    pub fn new(requests_per_second: u32) -> Self {
        let quota = Quota::per_second(
            NonZeroU32::new(requests_per_second).unwrap_or(NonZeroU32::new(10).unwrap()),
        );
        let limiter = RateLimiter::direct(quota);

        Self {
            limiter: Arc::new(limiter),
        }
    }

    /// Wait until a request is allowed
    pub async fn wait(&self) {
        self.limiter.until_ready().await;
        debug!("Rate limiter: request allowed");
    }

    /// Check if a request can be made immediately
    pub fn check(&self) -> bool {
        self.limiter.check().is_ok()
    }
}

impl Clone for GlobalRateLimiter {
    fn clone(&self) -> Self {
        Self {
            limiter: Arc::clone(&self.limiter),
        }
    }
}

impl Default for GlobalRateLimiter {
    fn default() -> Self {
        Self::new(10)
    }
}
