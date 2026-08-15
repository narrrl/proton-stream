//! Spacing requests out, so a provider does not have to.
//!
//! Scanning a new library is the one moment this app talks to a metadata
//! provider in bulk: every title is one search, a matched series is one more
//! request per season, and a hundred-folder share is several hundred requests
//! in a burst. AniList allows ninety a minute — thirty while its API is in the
//! degraded mode it has been in for a while — and answers everything past that
//! with a `429`. Without pacing, the first thirty titles match and the rest of
//! the run fails; because a failure is deliberately not cached (see
//! [`crate::service`]), the next scan starts over and fails in the same place.
//!
//! So requests take a slot here first. The rate is not a constant but the
//! provider's own: `X-RateLimit-Limit` on any answer retunes it, and a `429`
//! parks every caller for as long as `Retry-After` asks. One limiter is shared
//! by every lookup in flight — it lives in the provider, the provider lives in
//! an `Arc` inside [`crate::MetadataService`], and cloning the service to hand
//! it to a worker shares the limiter rather than copying it.

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Never faster than this, whatever a header claims.
const MIN_INTERVAL: Duration = Duration::from_millis(20);

/// Never slower than this. A provider that asks for a longer pause gets it via
/// [`RateLimiter::throttled`]; this is only the floor the steady rate decays to,
/// so a misread header cannot wedge a scan forever.
const MAX_INTERVAL: Duration = Duration::from_secs(5);

/// How long to honour a `429` that came with no `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(60);

/// The longest a single throttle may park requests for.
///
/// A scan is something a person is watching happen. Two minutes of silence is
/// worse than a partial answer they can ask to complete later, and the titles
/// that did not get through stay unmatched and askable.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A shared, self-tuning pacer for one provider's API.
pub struct RateLimiter {
    state: Mutex<State>,
}

struct State {
    /// The earliest a request may be sent.
    next: Instant,
    /// The spacing between requests.
    interval: Duration,
}

impl RateLimiter {
    /// A limiter starting at `per_minute` requests a minute.
    ///
    /// A starting point only: the first answer that carries a limit header
    /// replaces it with what the provider actually allows right now.
    pub fn per_minute(per_minute: u32) -> Self {
        Self::new(interval_for(per_minute.max(1)))
    }

    fn new(interval: Duration) -> Self {
        Self {
            state: Mutex::new(State {
                next: Instant::now(),
                interval,
            }),
        }
    }

    /// Wait until a request may be sent, then claim that slot.
    ///
    /// Re-checks after every sleep rather than reserving a slot up front, so a
    /// [`throttled`](Self::throttled) that lands while callers are already
    /// waiting holds them too — which is the whole point of a shared limiter
    /// during a burst.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.state.lock().await;
                let now = Instant::now();
                if state.next <= now {
                    state.next = now + state.interval;
                    return;
                }
                state.next - now
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// Retune from a provider's own rate-limit headers.
    pub async fn observe(&self, headers: &reqwest::header::HeaderMap) {
        let Some(limit) = header_number(headers, "x-ratelimit-limit") else {
            return;
        };
        let interval = interval_for(limit.max(1) as u32);
        let mut state = self.state.lock().await;
        if state.interval != interval {
            tracing::debug!("provider allows {limit}/min; pacing at {interval:?}");
            state.interval = interval;
        }
    }

    /// Park every caller after a `429`.
    ///
    /// `Retry-After` is seconds, and is the provider telling us exactly how
    /// long its window has left to run — a shorter guess would just earn
    /// another `429`.
    pub async fn throttled(&self, headers: &reqwest::header::HeaderMap) -> Duration {
        let wait = header_number(headers, "retry-after")
            .map(|seconds| Duration::from_secs(seconds.max(1)))
            .unwrap_or(DEFAULT_BACKOFF)
            .min(MAX_BACKOFF);

        let mut state = self.state.lock().await;
        let until = Instant::now() + wait;
        if until > state.next {
            state.next = until;
        }
        wait
    }
}

/// `90` a minute → one every 667ms.
fn interval_for(per_minute: u32) -> Duration {
    Duration::from_secs_f64(60.0 / f64::from(per_minute)).clamp(MIN_INTERVAL, MAX_INTERVAL)
}

fn header_number(headers: &reqwest::header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                value.parse().expect("header value"),
            );
        }
        map
    }

    #[test]
    fn a_rate_becomes_the_spacing_it_implies() {
        assert_eq!(interval_for(60), Duration::from_secs(1));
        assert_eq!(interval_for(30), Duration::from_secs(2));
        // Clamped at both ends: a provider claiming one request a minute must
        // not stall a scan for a minute per title, and one claiming a million
        // still gets paced.
        assert_eq!(interval_for(1), MAX_INTERVAL);
        assert_eq!(interval_for(1_000_000), MIN_INTERVAL);
    }

    #[tokio::test]
    async fn the_first_request_of_a_run_does_not_wait() {
        let limiter = RateLimiter::per_minute(30);
        let started = Instant::now();
        limiter.acquire().await;
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    /// The one that matters: two requests in a row are spaced, not sent
    /// together.
    #[tokio::test(start_paused = true)]
    async fn a_second_request_waits_out_the_interval() {
        let limiter = RateLimiter::per_minute(60);
        limiter.acquire().await;
        let started = Instant::now();
        limiter.acquire().await;
        assert!(started.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_parks_the_next_request_for_as_long_as_it_asked() {
        let limiter = RateLimiter::per_minute(600);
        let waited = limiter.throttled(&headers(&[("retry-after", "12")])).await;
        assert_eq!(waited, Duration::from_secs(12));

        let started = Instant::now();
        limiter.acquire().await;
        assert!(started.elapsed() >= Duration::from_secs(12));
    }

    /// A throttle with no header still has to park for something, and a
    /// ridiculous one is capped — a scan is something a person is watching.
    #[tokio::test]
    async fn a_429_without_a_header_backs_off_by_a_capped_default() {
        let limiter = RateLimiter::per_minute(90);
        assert_eq!(
            limiter.throttled(&headers(&[])).await,
            DEFAULT_BACKOFF.min(MAX_BACKOFF)
        );
        assert_eq!(
            limiter
                .throttled(&headers(&[("retry-after", "86400")]))
                .await,
            MAX_BACKOFF
        );
    }

    #[tokio::test]
    async fn the_provider_s_own_limit_retunes_the_pacing() {
        let limiter = RateLimiter::per_minute(90);
        limiter
            .observe(&headers(&[("x-ratelimit-limit", "30")]))
            .await;
        assert_eq!(
            limiter.state.lock().await.interval,
            Duration::from_secs(2),
            "AniList's degraded mode allows 30/min and says so on every answer"
        );

        // Nonsense, and an absent header, leave the pacing alone.
        limiter
            .observe(&headers(&[("x-ratelimit-limit", "soon")]))
            .await;
        limiter.observe(&headers(&[])).await;
        assert_eq!(limiter.state.lock().await.interval, Duration::from_secs(2));
    }
}
