//! Retry helper for LLM provider HTTP calls.
//!
//! Centralises the logic for re-trying a request after transient failures
//! (HTTP 429, 5xx, connection resets, timeouts, dropped streams). Exponential
//! backoff with jitter, capped at a small max so the agent doesn't appear hung.
//!
//! Only the **request submission** is retried, never the body of an in-flight
//! stream — once tokens have started flowing, retrying would either duplicate
//! tokens or corrupt the conversation. The provider modules call
//! [`should_retry_status`] to decide whether to bail or loop.

use std::time::Duration;

/// Default retry policy: 4 attempts (initial + 3 retries), 200ms base delay,
/// exponential factor 3x with up to ±50ms jitter, capped at 4s per backoff.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 4;
const BASE_DELAY_MS: u64 = 200;
const MAX_DELAY_MS: u64 = 4_000;
const JITTER_MS: u64 = 50;

/// Decide whether an HTTP status warrants a retry. 429 (rate limited), 408
/// (request timeout), and 5xx are retryable. Everything else is the caller's
/// responsibility to surface immediately.
pub fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

/// Decide whether a `reqwest::Error` from `send().await` warrants a retry.
/// Network errors (connection reset, DNS, timeout, broken pipe) are
/// retryable; HTTP-status errors are handled by [`should_retry_status`].
pub fn should_retry_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Compute the backoff for the n-th retry (0-indexed). Returns the duration
/// to sleep before attempt `n+1`.
pub fn backoff_for_attempt(attempt: u32) -> Duration {
    let base = BASE_DELAY_MS.saturating_mul(3u64.saturating_pow(attempt));
    let capped = base.min(MAX_DELAY_MS);
    // Trivial deterministic jitter source — we don't pull in the `rand` crate
    // for this; nanos-mod on a small range gives ±JITTER_MS variance which is
    // enough to break thundering herds across two clients.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let jitter = (nanos as u64) % (JITTER_MS * 2 + 1);
    let final_ms = capped.saturating_add(jitter).saturating_sub(JITTER_MS);
    Duration::from_millis(final_ms)
}

/// Send an HTTP request with retry. The closure must rebuild the request each
/// attempt because `RequestBuilder::send` consumes the builder. Returns the
/// final response (success, or terminal failure after retries).
///
/// Logs each retry via `tracing::warn` so users can see why their first call
/// "took a moment".
pub async fn send_with_retry<F, Fut>(
    provider: &'static str,
    mut build_send: F,
) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let mut attempt: u32 = 0;
    loop {
        let result = build_send().await;
        match result {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(resp);
                }
                if attempt + 1 >= DEFAULT_MAX_ATTEMPTS || !should_retry_status(status) {
                    return Ok(resp);
                }
                tracing::warn!(
                    target: "phazeai_core::llm::retry",
                    provider = provider,
                    status = %status,
                    attempt = attempt + 1,
                    "transient HTTP error; retrying after backoff"
                );
            }
            Err(e) => {
                if attempt + 1 >= DEFAULT_MAX_ATTEMPTS || !should_retry_error(&e) {
                    return Err(e);
                }
                tracing::warn!(
                    target: "phazeai_core::llm::retry",
                    provider = provider,
                    error = %e,
                    attempt = attempt + 1,
                    "transient network error; retrying after backoff"
                );
            }
        }
        tokio::time::sleep(backoff_for_attempt(attempt)).await;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn classifies_retryable_statuses() {
        assert!(should_retry_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(should_retry_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(should_retry_status(StatusCode::BAD_GATEWAY));
        assert!(should_retry_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(should_retry_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(should_retry_status(StatusCode::REQUEST_TIMEOUT));

        assert!(!should_retry_status(StatusCode::OK));
        assert!(!should_retry_status(StatusCode::UNAUTHORIZED));
        assert!(!should_retry_status(StatusCode::FORBIDDEN));
        assert!(!should_retry_status(StatusCode::NOT_FOUND));
        assert!(!should_retry_status(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn backoff_grows_then_caps() {
        let d0 = backoff_for_attempt(0).as_millis() as u64;
        let d1 = backoff_for_attempt(1).as_millis() as u64;
        let d2 = backoff_for_attempt(2).as_millis() as u64;
        let d10 = backoff_for_attempt(10).as_millis() as u64;
        // d1 should generally exceed d0 within jitter; d10 must be capped.
        assert!(d10 <= MAX_DELAY_MS + JITTER_MS);
        assert!(d0 <= 300, "d0 was {d0}");
        assert!((500..=700).contains(&d1), "d1 was {d1}");
        assert!((1700..=1900).contains(&d2), "d2 was {d2}");
    }
}
