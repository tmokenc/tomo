//! Tiny REST client.
//!
//! Every call goes through [`fetch_json`], which centralises:
//! * setting `credentials: include` so the session cookie is sent;
//! * mapping a 401 into [`ApiError::Unauthorized`] so callers can redirect
//!   to `/login` without inspecting status codes themselves.

use gloo_net::http::Request;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::types::*;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("http {0}: {1}")]
    Http(u16, String),
    #[error("network: {0}")]
    Network(String),
    #[error("decode: {0}")]
    Decode(String),
}

async fn fetch_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let resp = Request::get(path)
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    match resp.status() {
        200..=299 => resp.json::<T>().await.map_err(|e| ApiError::Decode(e.to_string())),
        401 => Err(ApiError::Unauthorized),
        403 => Err(ApiError::Forbidden),
        s => {
            let body = resp.text().await.unwrap_or_default();
            Err(ApiError::Http(s, body))
        }
    }
}

async fn post_json<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, ApiError> {
    let req = Request::post(path)
        .credentials(web_sys::RequestCredentials::SameOrigin)
        .json(body)
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let resp = req.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
    match resp.status() {
        200..=299 => resp.json::<T>().await.map_err(|e| ApiError::Decode(e.to_string())),
        401 => Err(ApiError::Unauthorized),
        403 => Err(ApiError::Forbidden),
        s => {
            let body = resp.text().await.unwrap_or_default();
            Err(ApiError::Http(s, body))
        }
    }
}

pub async fn me() -> Result<Me, ApiError> { fetch_json("/api/me").await }
pub async fn status() -> Result<Status, ApiError> { fetch_json("/api/status").await }
pub async fn global_stats() -> Result<GlobalStats, ApiError> { fetch_json("/api/stats/global").await }
pub async fn top_commands() -> Result<TopCommands, ApiError> { fetch_json("/api/stats/top").await }
pub async fn list_commands() -> Result<Vec<CommandInfo>, ApiError> { fetch_json("/api/commands").await }
pub async fn list_triggers() -> Result<Vec<TriggerInfo>, ApiError> { fetch_json("/api/triggers").await }

pub async fn reload_scripts() -> Result<ReloadResult, ApiError> {
    #[derive(serde::Serialize)]
    struct Empty {}
    post_json("/api/reload", &Empty {}).await
}
