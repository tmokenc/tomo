//! Error type shared across every LLM provider.

use std::time::Duration;

use thiserror::Error;

/// Errors returned from a [`crate::provider::Provider::generate`] call.
///
/// The router treats [`GenerateError::QuotaExceeded`] as a rotation signal —
/// the offending entry is parked for the carried `retry_after` and the next
/// (provider, model) entry is tried. Every other error short-circuits the
/// loop: a 401 / 403 / 5xx on one entry isn't going to magically pass on
/// the next, and silently rotating would mask real bugs.
#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("decode error: {0}")]
    Decode(String),
    /// 429 / `RESOURCE_EXHAUSTED`. `retry_after` is the server's hint (parsed
    /// from `Retry-After` / Google's `RetryInfo.retryDelay`) when present,
    /// otherwise a sensible default the caller chose.
    #[error("quota exhausted for {provider}/{model} (retry after {retry_after:?})")]
    QuotaExceeded {
        provider: String,
        model: String,
        retry_after: Duration,
    },
    #[error("{provider} api error ({status}): {body}")]
    Api {
        provider: String,
        status: u16,
        body: String,
    },
    #[error("client build failed: {0}")]
    Build(String),
    #[error("no providers configured — bot.llm is set up but the chain is empty")]
    NoProviders,
}
