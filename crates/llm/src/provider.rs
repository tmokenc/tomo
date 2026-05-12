//! `Provider` trait — one impl per upstream LLM API shape.
//!
//! Today we have two: the native Gemini REST API and an OpenAI-compatible
//! client that covers Groq, Cerebras, OpenRouter, Mistral, and anything
//! else exposing the standard `/v1/chat/completions` endpoint. The router
//! is shape-agnostic — it walks `Vec<RouteEntry>` and treats every entry
//! the same.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::conversation::Turn;
use crate::error::GenerateError;

/// What [`Provider::generate`] returns. The router stamps the `provider` /
/// `model` fields onto this when it propagates the response, so callers
/// always know which entry served the request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub text: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
}

/// Single backend the router can dispatch to.
///
/// Implementations should be cheap to clone (typically `Arc<reqwest::Client>`
/// inside) — the router stores them as `Arc<dyn Provider>` and re-uses
/// connections.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Short identifier for logs / cooldown keys. Lower-case, alphanumeric:
    /// `"gemini"`, `"groq"`, `"cerebras"`, `"openrouter"`, `"mistral"`.
    fn name(&self) -> &str;

    /// Send a single completion request.
    ///
    /// * `model` — which model on this provider to use. The router selects
    ///   the entry; the provider just forwards the id to its API.
    /// * `turns` — the conversation so far, **ending in a user turn**.
    /// * `system` — system instruction. Already includes any per-request
    ///   context the caller wanted to append.
    /// * `max_output_tokens` — generation cap, mirrored from config.
    async fn generate(
        &self,
        model: &str,
        turns: &[Turn],
        system: &str,
        max_output_tokens: u32,
    ) -> Result<ProviderResponse, GenerateError>;

    /// Cheap reachability probe. The default is a no-op (`Ok`) — concrete
    /// providers override when they have a free `GET /models` style endpoint
    /// that doesn't burn a request from the free quota.
    async fn health_check(&self) -> Result<(), GenerateError> {
        Ok(())
    }
}
