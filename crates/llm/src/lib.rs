//! Multi-provider LLM router for the bot.
//!
//! Despite the crate's historical name (`tomo-llm`), this is now a
//! provider-agnostic router. Today it supports Google's native Gemini API
//! and any OpenAI-compatible service (Groq, Cerebras, OpenRouter, Mistral,
//! …). Adding another shape is implementing one `Provider` trait method.
//!
//! Entry point: [`LlmRouter`]. The discord crate constructs one at startup
//! from the configured chain and asks it for completions on every
//! mention.

pub mod conversation;
pub mod error;
pub mod provider;
pub mod providers;
pub mod ratelimit;
pub mod router;

pub use conversation::{ConversationStore, Role, Turn};
pub use error::GenerateError;
pub use provider::{Provider, ProviderResponse};
pub use providers::{GeminiProvider, OpenAiCompatKind, OpenAiCompatProvider};
pub use ratelimit::RateLimiter;
pub use router::{GenerateResponse, LlmRouter, RouteEntry};
