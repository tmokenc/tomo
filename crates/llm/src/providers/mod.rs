//! Concrete [`crate::provider::Provider`] implementations.
//!
//! * `gemini` — Google's native `generateContent` REST endpoint.
//! * `openai_compat` — OpenAI-style `POST /chat/completions`. Works for
//!   Groq, Cerebras, OpenRouter, Mistral, Together, Cloudflare Workers AI,
//!   and any other provider that ships the same shape.

pub mod gemini;
pub mod openai_compat;

pub use gemini::GeminiProvider;
pub use openai_compat::{OpenAiCompatKind, OpenAiCompatProvider};
