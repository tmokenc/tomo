use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use tomo_core::error::{Error, Result};

/// Settings the admin service needs that are independent of the bot config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminConfig {
    /// Address to bind. Defaults to `127.0.0.1:8080` — do not change unless
    /// you've already put a TLS-terminating reverse proxy in front.
    pub bind: SocketAddr,

    /// Where the gRPC server lives. Should match what the bot exposes.
    pub rpc_url: String,

    /// Bearer token for the gRPC connection. Optional, but required when
    /// the bot has one configured.
    pub rpc_token: Option<String>,

    /// Discord OAuth client id (from the developer portal).
    pub oauth_client_id: String,

    /// Discord OAuth client secret.
    pub oauth_client_secret: String,

    /// Where Discord should redirect after authorising. Must exactly match
    /// what's configured in the Discord dev portal.
    pub oauth_redirect_url: String,

    /// HMAC key for signing cookies. 32+ bytes from a CSPRNG. Tomo refuses to
    /// start in production if this is shorter than 32 bytes.
    pub session_secret: String,

    /// Session lifetime.
    pub session_ttl: chrono::Duration,

    /// Directory containing the compiled front-end (`index.html`, `*.wasm`,
    /// `*.js`, …). Produced by `trunk build` in `frontend/`.
    pub frontend_dist: PathBuf,

    /// If true, the response includes `Strict-Transport-Security`. Disable
    /// for HTTP-only local development; default is `true`.
    pub enable_hsts: bool,
}

impl AdminConfig {
    pub fn from_env() -> Result<Self> {
        let bind = optional("TOMO_ADMIN_BIND")
            .as_deref()
            .map(SocketAddr::from_str)
            .transpose()
            .map_err(|e| Error::config(format!("TOMO_ADMIN_BIND: {e}")))?
            .unwrap_or_else(|| "127.0.0.1:8080".parse().unwrap());

        let rpc_url = optional("TOMO_RPC_URL").unwrap_or_else(|| "http://127.0.0.1:50051".into());
        let rpc_token = optional("TOMO_RPC_TOKEN");

        let oauth_client_id = require("DISCORD_OAUTH_CLIENT_ID")?;
        let oauth_client_secret = require("DISCORD_OAUTH_CLIENT_SECRET")?;
        let oauth_redirect_url = require("DISCORD_OAUTH_REDIRECT_URL")?;

        let session_secret = require("TOMO_ADMIN_SESSION_SECRET")?;
        if session_secret.len() < 32 {
            return Err(Error::config(
                "TOMO_ADMIN_SESSION_SECRET must be at least 32 bytes long",
            ));
        }

        let session_ttl_hours: i64 = optional("TOMO_ADMIN_SESSION_HOURS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(24);
        let session_ttl = chrono::Duration::hours(session_ttl_hours.clamp(1, 24 * 7));

        let frontend_dist = optional("TOMO_ADMIN_FRONTEND")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("frontend/dist"));

        let enable_hsts = optional("TOMO_ADMIN_ENABLE_HSTS")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(true);

        Ok(Self {
            bind,
            rpc_url,
            rpc_token,
            oauth_client_id,
            oauth_client_secret,
            oauth_redirect_url,
            session_secret,
            session_ttl,
            frontend_dist,
            enable_hsts,
        })
    }

    /// Whether to enable the admin service at all. Convenience wrapper around
    /// `TOMO_ENABLE_ADMIN` so callers don't need to call `from_env` first.
    pub fn enabled() -> bool {
        optional("TOMO_ENABLE_ADMIN")
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
    }
}

fn require(key: &'static str) -> Result<String> {
    env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or(Error::MissingEnv(key))
}

fn optional(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}
