//! Pacing for outbound calls to one metadata source: unpaced, a first pass over a large library
//! trips the `enrichment` breaker on a source that is up and has a limit.
//!
//! A reservation token bucket: a caller takes its token under the lock, then sleeps outside
//! it. The balance goes negative so each waiter is told a different instant, in arrival order.

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
    /// How long the next refusal that names no wait holds everything back.
    back_off: Duration,
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
                back_off: FIRST_BACK_OFF,
            })),
            capacity,
        }
    }

    /// A limiter that never delays, for sources with no meaningful limit.
    pub fn unlimited() -> Self {
        Self::new(u32::MAX, u32::MAX)
    }

    /// Wait until this caller may issue its request. A hold placed while it
    /// slept ends after the instant it was given, so it takes a place after
    /// the hold, or it would go out while the source still refuses.
    pub async fn acquire(&self) {
        loop {
            let wait = self.reserve().await;
            if wait.is_zero() {
                return;
            }
            tokio::time::sleep(wait).await;
            if self.state.lock().await.last_refill <= Instant::now() {
                return;
            }
        }
    }

    /// [`RateLimiter::acquire`] for a caller with a deadline: `false` at once, its turn given
    /// back, when the turn falls past `deadline`, a hold placed while it slept included.
    pub async fn acquire_by(&self, deadline: Instant) -> bool {
        loop {
            let wait = self.reserve().await;
            if wait.is_zero() {
                return true;
            }
            if Instant::now() + wait > deadline {
                self.state.lock().await.tokens += 1.0;
                return false;
            }
            tokio::time::sleep(wait).await;
            if self.state.lock().await.last_refill <= Instant::now() {
                return true;
            }
        }
    }

    /// Take a token and report how long its holder must wait, apart from `acquire` so a test
    /// reads the wait without sleeping through it.
    async fn reserve(&self) -> Duration {
        let mut state = self.state.lock().await;
        let now = Instant::now();

        if now > state.last_refill {
            let elapsed = (now - state.last_refill).as_secs_f64();
            state.tokens = (state.tokens + elapsed * state.rate).min(self.capacity);
            state.last_refill = now;
        }

        // Counted from the later of now and a `Retry-After`'s moment, so the waiters it held
        // leave at the source's rate rather than all at once.
        state.tokens -= 1.0;
        let owed = Duration::from_secs_f64((-state.tokens).max(0.0) / state.rate);
        (state.last_refill + owed).saturating_duration_since(now)
    }

    /// Hold everything back for `delay`, because the source asked, and let one
    /// request out at its end, the next at the source's rate after it.
    pub async fn penalise(&self, delay: Duration) {
        hold(&mut *self.state.lock().await, Instant::now() + delay);
    }

    /// Hold everything back after a refusal that named no wait, longer each time the source
    /// still refuses once a hold has ended. A refusal during a hold answers a request sent
    /// before it, and lengthens nothing.
    pub async fn back_off(&self) {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        if state.last_refill > now {
            return;
        }
        let held = state.back_off;
        state.back_off = (held * 2).min(Duration::from_secs(MAX_HOLD_SECS));
        hold(&mut state, now + held);
    }

    /// The source answered: its next refusal backs off from the start again.
    async fn answered(&self) {
        self.state.lock().await.back_off = FIRST_BACK_OFF;
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

/// The longest a source's own word, or a back-off, holds every request back.
const MAX_HOLD_SECS: u64 = 300;

/// The first back-off after a refusal that named no wait.
const FIRST_BACK_OFF: Duration = Duration::from_secs(30);

/// Hold everything back until `until`, letting one request out then. Only
/// ever extends: two concurrent refusals must not let the shorter one shorten
/// the longer one's wait.
fn hold(state: &mut State, until: Instant) {
    if until > state.last_refill {
        state.last_refill = until;
        state.tokens = state.tokens.min(1.0);
    }
}

/// Pace `limiter` after an answer of its source: a `Retry-After` holds every request back for
/// its wait, and a 429 naming none backs off.
pub async fn after_answer<T>(limiter: &RateLimiter, outcome: &crate::error::AppResult<T>) {
    use crate::error::AppError;
    match outcome {
        Ok(_) => limiter.answered().await,
        Err(AppError::ExternalApi { retry_after: Some(seconds), service, .. }) => {
            tracing::warn!(
                "{service} asked for {seconds}s before the next request, pacing after it"
            );
            limiter.penalise(Duration::from_secs(*seconds)).await;
        }
        Err(AppError::ExternalApi { status: 429, service, .. }) => {
            tracing::warn!("{service} refused a request as too many, backing off");
            limiter.back_off().await;
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_burst_is_allowed_before_any_pacing_begins() {
        let limiter = RateLimiter::new(60, 3);

        for _ in 0..3 {
            assert_eq!(limiter.reserve().await, Duration::ZERO);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn past_the_burst_requests_are_spaced_by_the_rate() {
        let limiter = RateLimiter::new(60, 1);
        assert_eq!(limiter.reserve().await, Duration::ZERO);

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
    async fn a_waiter_reserved_before_a_penalty_waits_for_its_end() {
        let limiter = RateLimiter::new(60, 1);
        limiter.acquire().await;
        let waiter = tokio::spawn({
            let limiter = limiter.clone();
            async move {
                limiter.acquire().await;
                Instant::now()
            }
        });
        tokio::task::yield_now().await;

        let refused = Instant::now();
        limiter.penalise(Duration::from_secs(30)).await;

        let sent = waiter.await.unwrap();
        assert!(sent - refused >= Duration::from_secs(30), "sent {:?} in", sent - refused);
    }

    /// A caller with a deadline gives up at once when its turn falls past it, and the turn it
    /// gave up goes to the next caller.
    #[tokio::test(start_paused = true)]
    async fn acquire_by_gives_up_at_once_past_its_deadline() {
        let limiter = RateLimiter::new(60, 1);
        limiter.penalise(Duration::from_secs(60)).await;
        let start = Instant::now();

        assert!(!limiter.acquire_by(start + Duration::from_secs(1)).await, "waited past it");
        assert_eq!(Instant::now(), start, "time passed while it gave up");
        assert_eq!(limiter.reserve().await.as_secs(), 60, "the turn given up was kept");
        assert!(limiter.acquire_by(start + Duration::from_secs(120)).await);
    }

    fn too_many() -> crate::error::AppResult<()> {
        Err(crate::integrations::upstream(
            "AniList",
            429,
            crate::error::Upstream::Http,
            String::new(),
        ))
    }

    /// The second refusal of each round lands during the hold and lengthens nothing.
    #[tokio::test(start_paused = true)]
    async fn a_429_without_retry_after_still_slows_the_source() {
        let limiter = RateLimiter::unlimited();
        let mut holds = Vec::new();
        for _ in 0..6 {
            after_answer(&limiter, &too_many()).await;
            after_answer(&limiter, &too_many()).await;
            let wait = limiter.reserve().await;
            holds.push(wait.as_secs());
            tokio::time::advance(wait).await;
        }
        assert_eq!(holds, [30, 60, 120, 240, 300, 300]);

        after_answer(&limiter, &Ok(())).await;
        after_answer(&limiter, &too_many()).await;
        assert_eq!(limiter.reserve().await.as_secs(), 30);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unlimited_source_is_never_delayed() {
        let limiter = RateLimiter::unlimited();
        for _ in 0..1000 {
            assert_eq!(limiter.reserve().await, Duration::ZERO);
        }
    }
}
