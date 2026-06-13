//! Error type shared across every LLM provider.

use std::time::Duration;

use thiserror::Error;

/// Errors returned from a [`crate::provider::Provider::generate`] call.
///
/// The router treats *every* error as a rotation signal: park the offending
/// (provider, model) entry for [`GenerateError::cooldown_hint`] and try the
/// next one. Only when the entire chain refuses does the router give up and
/// surface the last error to the caller. The previous design short-circuited
/// on anything that wasn't a 429 — but real free-tier endpoints return
/// per-model 404s (e.g. Cerebras `model_not_found` for paid-only models),
/// 5xx blips, and transient transport errors that *do* differ from one
/// entry to the next, so a wider net is correct.
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

impl GenerateError {
    /// How long the router should park the offending (provider, model)
    /// before trying it again. Capped by the router's own `MAX_COOLDOWN`.
    ///
    /// Per-class reasoning:
    /// * **QuotaExceeded** — use the server's hint as-is.
    /// * **4xx** (other than 429) — the request shape is rejected; very
    ///   long park (the model is unlikely to start accepting us during
    ///   this session). Covers `model_not_found`, auth refusals, etc.
    /// * **5xx** — provider blip; park briefly so an in-flight wave of
    ///   requests doesn't all retry at once, but allow recovery.
    /// * **Transport / Decode** — likely transient; park for just long
    ///   enough that we don't immediately re-pick the same flaky entry.
    pub fn cooldown_hint(&self) -> Duration {
        match self {
            Self::QuotaExceeded { retry_after, .. } => *retry_after,
            Self::Api { status, .. } if (400..500).contains(status) => {
                Duration::from_secs(60 * 60) // 1h — effectively "park for the session"
            }
            Self::Api { .. } => Duration::from_secs(60),
            Self::Transport(_) | Self::Decode(_) => Duration::from_secs(15),
            Self::Build(_) | Self::NoProviders => Duration::from_secs(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_uses_server_hint_for_quota() {
        let e = GenerateError::QuotaExceeded {
            provider: "groq".into(),
            model: "x".into(),
            retry_after: Duration::from_secs(42),
        };
        assert_eq!(e.cooldown_hint(), Duration::from_secs(42));
    }

    #[test]
    fn cooldown_for_4xx_is_session_long() {
        let e = GenerateError::Api {
            provider: "cerebras".into(),
            status: 404,
            body: "model_not_found".into(),
        };
        // 1h — effectively "don't bother this session".
        assert_eq!(e.cooldown_hint(), Duration::from_secs(3600));
    }

    #[test]
    fn cooldown_for_5xx_is_short() {
        let e = GenerateError::Api {
            provider: "groq".into(),
            status: 503,
            body: "upstream".into(),
        };
        assert_eq!(e.cooldown_hint(), Duration::from_secs(60));
    }

    #[test]
    fn cooldown_for_transport_is_tiny() {
        let e = GenerateError::Transport("connection reset".into());
        assert_eq!(e.cooldown_hint(), Duration::from_secs(15));
    }
}
