//! Pacing for outbound calls to one metadata source.
//!
//! The circuit breaker in `enrichment` stops a pass hammering a source that is
//! *down*. It does nothing about a source that is up and simply has a limit:
//! AniList allows roughly 90 requests a minute and Jikan 60, and a concurrency
//! cap is not a rate — four requests in flight can still mean forty a second if
//! each one is fast.
//!
//! Without pacing, a first pass over a large library trips its own breaker,
//! abandons the source, and has to be repeated until it converges. With it, the
//! pass takes the time the source's limit implies and finishes.
//!
//! A **reservation** token bucket: a caller takes its token under the lock, then
//! sleeps outside it. Letting the balance go negative is what makes the queue
//! fair — each waiter is told a different instant to wake at, in the order it
//! arrived, instead of every waiter racing for the same moment.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Debug)]
struct State {
    /// Available tokens. Negative means requests are queued ahead of this one.
    tokens: f64,
    last_refill: Instant,
    /// Set by `penalise`, when a source states how long it wants to be left
    /// alone. Nothing goes out before this, whatever the bucket says.
    not_before: Option<Instant>,
}

/// Paces requests to one source. Cloning shares the same allowance, which is
/// what makes it usable from every future of a `buffer_unordered` pass.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<State>>,
    /// Tokens added per second.
    rate: f64,
    /// How many may be spent at once after an idle period. A small library
    /// should not be slowed to the sustained rate it will never reach.
    capacity: f64,
}

impl RateLimiter {
    /// `per_minute` is the sustained ceiling; `burst` what an idle bucket holds.
    pub fn new(per_minute: u32, burst: u32) -> Self {
        let capacity = burst.max(1) as f64;
        Self {
            state: Arc::new(Mutex::new(State {
                tokens: capacity,
                last_refill: Instant::now(),
                not_before: None,
            })),
            rate: (per_minute.max(1) as f64) / 60.0,
            capacity,
        }
    }

    /// A limiter that never delays, for sources with no meaningful limit.
    pub fn unlimited() -> Self {
        Self::new(u32::MAX, u32::MAX)
    }

    /// Wait until this caller may issue its request.
    pub async fn acquire(&self) {
        let wait = self.reserve().await;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Take a token and report how long its holder must wait for it.
    ///
    /// Separate from `acquire` so the arithmetic can be tested without actually
    /// sleeping through it.
    async fn reserve(&self) -> Duration {
        let mut state = self.state.lock().await;
        let now = Instant::now();

        let elapsed = now.saturating_duration_since(state.last_refill).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.rate).min(self.capacity);
        state.last_refill = now;

        state.tokens -= 1.0;
        let bucket_wait = if state.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-state.tokens / self.rate)
        };

        // An explicit `Retry-After` outranks the bucket: the source knows
        // something about its own state that arithmetic here cannot.
        let penalty_wait = state
            .not_before
            .map(|until| until.saturating_duration_since(now))
            .unwrap_or(Duration::ZERO);

        bucket_wait.max(penalty_wait)
    }

    /// Hold everything back for `delay`, because the source asked.
    ///
    /// Only ever extends: two concurrent 429s must not let the shorter one
    /// shorten the longer one's wait.
    pub async fn penalise(&self, delay: Duration) {
        let mut state = self.state.lock().await;
        let until = Instant::now() + delay;
        if state.not_before.is_none_or(|current| until > current) {
            state.not_before = Some(until);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_burst_is_allowed_before_any_pacing_begins() {
        let limiter = RateLimiter::new(60, 3);

        // Three tokens are in the bucket, so the first three cost nothing: a
        // three-item library must not be paced to a rate it never reaches.
        for _ in 0..3 {
            assert_eq!(limiter.reserve().await, Duration::ZERO);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn past_the_burst_requests_are_spaced_by_the_rate() {
        let limiter = RateLimiter::new(60, 1);
        assert_eq!(limiter.reserve().await, Duration::ZERO);

        // 60 a minute is one a second.
        let second = limiter.reserve().await;
        assert!(
            (second.as_secs_f64() - 1.0).abs() < 0.01,
            "expected a one second wait, got {second:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waiters_are_queued_rather_than_all_woken_at_once() {
        let limiter = RateLimiter::new(60, 1);
        limiter.reserve().await;

        // Each waiter is told a later instant than the one before it. Without
        // the balance going negative they would all be told "one second" and
        // then stampede the source together.
        let first = limiter.reserve().await;
        let second = limiter.reserve().await;
        let third = limiter.reserve().await;

        assert!(second > first, "{second:?} should be later than {first:?}");
        assert!(third > second, "{third:?} should be later than {second:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn tokens_come_back_while_the_bucket_sits_idle() {
        let limiter = RateLimiter::new(60, 2);
        limiter.reserve().await;
        limiter.reserve().await;

        tokio::time::advance(Duration::from_secs(5)).await;

        // Refilled — and capped at the burst, not five seconds' worth.
        assert_eq!(limiter.reserve().await, Duration::ZERO);
        assert_eq!(limiter.reserve().await, Duration::ZERO);
        assert!(limiter.reserve().await > Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn a_retry_after_outranks_the_bucket() {
        let limiter = RateLimiter::new(6000, 100);
        // The bucket alone would let this through immediately.
        assert_eq!(limiter.reserve().await, Duration::ZERO);

        limiter.penalise(Duration::from_secs(30)).await;

        let wait = limiter.reserve().await;
        assert!(wait >= Duration::from_secs(29), "the source's own delay was ignored: {wait:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_shorter_penalty_never_shortens_a_longer_one() {
        let limiter = RateLimiter::unlimited();
        limiter.penalise(Duration::from_secs(60)).await;
        limiter.penalise(Duration::from_secs(1)).await;

        let wait = limiter.reserve().await;
        assert!(wait >= Duration::from_secs(59), "a second 429 cut the first one short: {wait:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unlimited_source_is_never_delayed() {
        let limiter = RateLimiter::unlimited();
        for _ in 0..1000 {
            assert_eq!(limiter.reserve().await, Duration::ZERO);
        }
    }
}
