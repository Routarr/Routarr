//! Pacing for outbound calls to one metadata source.
//!
//! The circuit breaker in `enrichment` stops a pass hammering a source that is
//! *down*. It does nothing about a source that is up and simply has a limit:
//! AniList allows 30 requests a minute while degraded and Jikan 60, and a
//! concurrency cap is not a rate: four requests in flight can still mean forty
//! a second if each one is fast.
//!
//! Without pacing, a first pass over a large library trips its own breaker,
//! abandons the source, and has to be repeated until it converges. With it, the
//! pass takes the time the source's limit implies and finishes.
//!
//! A **reservation** token bucket: a caller takes its token under the lock, then
//! sleeps outside it. Letting the balance go negative is what makes the queue
//! fair: each waiter is told a different instant to wake at, in the order it
//! arrived, instead of every waiter racing for the same moment.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::integrations::StatedLimit;

#[derive(Debug)]
struct State {
    /// Available tokens. Negative means requests are queued ahead of this one.
    tokens: f64,
    /// When the bucket last refilled, or, after a `Retry-After`, the moment the
    /// source named: in the future, nothing refills and nothing goes out
    /// before it.
    last_refill: Instant,
    /// Tokens added per second, which the source's stated limit retunes.
    rate: f64,
}

/// Paces requests to one source. Cloning shares the same allowance, which is
/// what makes it usable from every future of a `buffer_unordered` pass.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<State>>,
    /// How many may be spent at once after an idle period. A small library
    /// should not be slowed to the sustained rate it will never reach.
    capacity: f64,
}

impl RateLimiter {
    /// `per_minute` is the sustained ceiling, `burst` what an idle bucket holds.
    pub fn new(per_minute: u32, burst: u32) -> Self {
        let capacity = burst.max(1) as f64;
        let rate = (per_minute.max(1) as f64) / 60.0;
        Self {
            state: Arc::new(Mutex::new(State {
                tokens: capacity,
                last_refill: Instant::now(),
                rate,
            })),
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

        if now > state.last_refill {
            let elapsed = (now - state.last_refill).as_secs_f64();
            state.tokens = (state.tokens + elapsed * state.rate).min(self.capacity);
            state.last_refill = now;
        }

        // Counted from the later of now and the moment a `Retry-After` named,
        // so the waiters it held leave at the source's rate from that moment
        // rather than all at once.
        state.tokens -= 1.0;
        let owed = Duration::from_secs_f64((-state.tokens).max(0.0) / state.rate);
        (state.last_refill + owed).saturating_duration_since(now)
    }

    /// Hold everything back for `delay`, because the source asked, and let one
    /// request out at its end, the next at the source's rate after it.
    pub async fn penalise(&self, delay: Duration) {
        hold(&mut *self.state.lock().await, Instant::now() + delay);
    }

    /// Pace to the limit the source states: its rate, no more requests than it
    /// says remain, and none before its reset once none remain. A reset
    /// further than five minutes away is held to five, as a `Retry-After` is.
    pub async fn follow(&self, stated: StatedLimit) {
        let mut state = self.state.lock().await;
        if let Some(per_minute) = stated.per_minute.filter(|per_minute| *per_minute > 0) {
            state.rate = f64::from(per_minute) / 60.0;
        }
        if let Some(remaining) = stated.remaining {
            state.tokens = state.tokens.min(f64::from(remaining));
        }
        if let (Some(0), Some(reset)) = (stated.remaining, stated.reset) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let wait = Duration::from_secs(reset.saturating_sub(now).min(MAX_HOLD_SECS));
            hold(&mut state, Instant::now() + wait);
        }
    }
}

/// The longest a source's own word holds every request back.
const MAX_HOLD_SECS: u64 = 300;

/// Hold everything back until `until`, letting one request out then. Only
/// ever extends: two concurrent refusals must not let the shorter one shorten
/// the longer one's wait.
fn hold(state: &mut State, until: Instant) {
    if until > state.last_refill {
        state.last_refill = until;
        state.tokens = state.tokens.min(1.0);
    }
}

/// Hold the limiter back when a source states how long it wants to be left
/// alone.
///
/// Pacing is a guess about someone else's limit. `Retry-After` is that someone
/// telling us. When it arrives, every later request slows to match instead of
/// spending its budget discovering the same thing again.
pub async fn honour_retry_after<T>(limiter: &RateLimiter, outcome: &crate::error::AppResult<T>) {
    if let Err(crate::error::AppError::ExternalApi {
        retry_after: Some(seconds), service, ..
    }) = outcome
    {
        tracing::warn!("{service} asked for {seconds}s before the next request, pacing after it");
        limiter.penalise(Duration::from_secs(*seconds)).await;
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

        // Refilled, but capped at the burst rather than five seconds' worth.
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

    /// After a `Retry-After`, the waiters leave one at a time at the source's
    /// rate, from the moment it named, not all together at that moment.
    #[tokio::test(start_paused = true)]
    async fn the_waiters_held_by_a_retry_after_leave_paced() {
        let limiter = RateLimiter::new(60, 5);
        limiter.penalise(Duration::from_secs(30)).await;

        let mut waits = Vec::new();
        for _ in 0..3 {
            waits.push(limiter.reserve().await.as_secs());
        }

        assert_eq!(waits, [30, 31, 32]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_shorter_penalty_never_shortens_a_longer_one() {
        let limiter = RateLimiter::unlimited();
        limiter.penalise(Duration::from_secs(60)).await;
        limiter.penalise(Duration::from_secs(1)).await;

        let wait = limiter.reserve().await;
        assert!(wait >= Duration::from_secs(59), "a second 429 cut the first one short: {wait:?}");
    }

    /// The rate a source states replaces the one assumed, no more requests
    /// go out than it says remain, and none before its reset once none do.
    #[tokio::test(start_paused = true)]
    async fn the_limit_a_source_states_is_followed() {
        let limiter = RateLimiter::new(30, 5);
        limiter.follow(StatedLimit { per_minute: Some(90), remaining: Some(1), reset: None }).await;
        assert_eq!(limiter.reserve().await, Duration::ZERO);
        let paced = limiter.reserve().await.as_secs_f64();
        assert!((paced - 60.0 / 90.0).abs() < 0.01, "not paced at 90 a minute: {paced}");

        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        let reset = Some(now.as_secs() + 40);
        limiter.follow(StatedLimit { per_minute: None, remaining: Some(0), reset }).await;
        let held = limiter.reserve().await;
        assert!(held >= Duration::from_secs(38), "the reset was not waited for: {held:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unlimited_source_is_never_delayed() {
        let limiter = RateLimiter::unlimited();
        for _ in 0..1000 {
            assert_eq!(limiter.reserve().await, Duration::ZERO);
        }
    }
}
