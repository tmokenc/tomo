//! Minimal client for the Gemini `generateContent` REST endpoint.
//!
//! The bot uses this when a non-bot user `@`-mentions Tomo. The client tracks
//! short per-channel chat history so replies feel conversational, and a
//! per-user rate limit prevents abuse.

pub mod client;
pub mod conversation;
pub mod ratelimit;

pub use client::{GeminiClient, GenerateError, GenerateResponse, HealthInfo};
pub use conversation::{ConversationStore, Role, Turn};
pub use ratelimit::RateLimiter;
