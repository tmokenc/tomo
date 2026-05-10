//! Admin web service for Tomo.
//!
//! Provides:
//! * Discord OAuth login (auth code + PKCE + `state`).
//! * Signed, HttpOnly session cookie restricted to bot owners.
//! * JSON REST API consumed by the Yew SPA front-end.
//! * Static file serving for the compiled front-end bundle.
//! * gRPC client into the bot ([`tomo_rpc::TomoControlClient`]).
//!
//! Security defaults:
//! * Bind to `127.0.0.1:8080` — do not expose without a TLS reverse proxy.
//! * `SameSite=Lax` + `Secure` + `HttpOnly` session cookies.
//! * `Strict-Transport-Security`, `X-Content-Type-Options`,
//!   `Referrer-Policy`, `X-Frame-Options: DENY` on every response.
//! * `Content-Security-Policy` allows scripts/styles only from `self` (and
//!   a CDN allow-list for Chart.js).

pub mod api;
pub mod config;
pub mod middleware;
pub mod oauth;
pub mod security;
pub mod service;
pub mod session;
pub mod state;
pub mod static_files;

pub use config::AdminConfig;
pub use service::AdminService;
