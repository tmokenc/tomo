//! JSON API for the Yew front-end.
//!
//! Every handler is gated by [`crate::middleware::require_owner`] so an
//! authenticated bot owner is guaranteed.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Serialize;
use tonic::Request as TonicRequest;
use tracing::warn;

use tomo_rpc::{Empty, GuildStatsRequest, ReloadRequest, TopCommandsRequest, UserStatsRequest};

use crate::session::Session;
use crate::state::AppState;

// ---- Response shapes (kept independent of the proto types so the public
// API can evolve without breaking gRPC clients) ----

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub name: String,
    pub version: String,
    pub started_at_unix: i64,
    pub uptime_seconds: i64,
    pub owners: Vec<u64>,
    pub bot_user_id: u64,
    pub llm_enabled: bool,
    pub discriminator: u32,
    pub avatar_url: String,
    pub application_id: u64,
    pub application_name: String,
    pub application_description: String,
    pub bot_public: bool,
    pub bot_require_code_grant: bool,
    pub verified: bool,
    pub owner_id: u64,
    pub owner_username: String,
}

#[derive(Debug, Serialize)]
pub struct GlobalStatsResponse {
    pub messages: u64,
    pub commands: u64,
    pub llm_calls: u64,
    pub llm_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct UserStatsResponse {
    pub user_id: u64,
    pub messages: u64,
    pub commands: u64,
    pub llm_calls: u64,
    pub llm_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct GuildStatsResponse {
    pub guild_id: u64,
    pub messages: u64,
    pub commands: u64,
}

#[derive(Debug, Serialize)]
pub struct TopCommandsResponse {
    pub commands: Vec<TopCommandEntry>,
}

#[derive(Debug, Serialize)]
pub struct TopCommandEntry {
    pub command: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct CommandInfoDto {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub slash: bool,
    pub prefix: bool,
    pub guild_only: bool,
    pub owner_only: bool,
    pub is_script: bool,
}

#[derive(Debug, Serialize)]
pub struct TriggerInfoDto {
    pub name: String,
    pub matcher_kind: String,
    pub matcher_arg: String,
    pub is_script: bool,
}

#[derive(Debug, Serialize)]
pub struct ReloadResponse {
    pub success: bool,
    pub message: String,
    pub command_count: u32,
    pub trigger_count: u32,
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id: u64,
    pub username: String,
    pub expires_at_unix: i64,
}

// ---- Handlers ----

pub async fn me(Extension(session): Extension<Session>) -> Json<MeResponse> {
    Json(MeResponse {
        user_id: session.user_id,
        username: session.username,
        expires_at_unix: session.expires_at.timestamp(),
    })
}

pub async fn status(State(state): State<AppState>) -> Result<Json<StatusResponse>, ApiError> {
    let resp = rpc_call(&state, |mut c| async move { c.get_status(TonicRequest::new(Empty {})).await })
        .await?
        .into_inner();
    Ok(Json(StatusResponse {
        name: resp.name,
        version: resp.version,
        started_at_unix: resp.started_at_unix,
        uptime_seconds: resp.uptime_seconds,
        owners: resp.owners,
        bot_user_id: resp.bot_user_id,
        llm_enabled: resp.llm_enabled,
        discriminator: resp.discriminator,
        avatar_url: resp.avatar_url,
        application_id: resp.application_id,
        application_name: resp.application_name,
        application_description: resp.application_description,
        bot_public: resp.bot_public,
        bot_require_code_grant: resp.bot_require_code_grant,
        verified: resp.verified,
        owner_id: resp.owner_id,
        owner_username: resp.owner_username,
    }))
}

pub async fn global_stats(State(state): State<AppState>) -> Result<Json<GlobalStatsResponse>, ApiError> {
    let resp = rpc_call(&state, |mut c| async move {
        c.get_global_stats(TonicRequest::new(Empty {})).await
    })
    .await?
    .into_inner();
    Ok(Json(GlobalStatsResponse {
        messages: resp.messages,
        commands: resp.commands,
        llm_calls: resp.llm_calls,
        llm_tokens: resp.llm_tokens,
    }))
}

pub async fn user_stats(
    State(state): State<AppState>,
    Path(user_id): Path<u64>,
) -> Result<Json<UserStatsResponse>, ApiError> {
    let resp = rpc_call(&state, move |mut c| async move {
        c.get_user_stats(TonicRequest::new(UserStatsRequest { user_id })).await
    })
    .await?
    .into_inner();
    Ok(Json(UserStatsResponse {
        user_id: resp.user_id,
        messages: resp.messages,
        commands: resp.commands,
        llm_calls: resp.llm_calls,
        llm_tokens: resp.llm_tokens,
    }))
}

pub async fn guild_stats(
    State(state): State<AppState>,
    Path(guild_id): Path<u64>,
) -> Result<Json<GuildStatsResponse>, ApiError> {
    let resp = rpc_call(&state, move |mut c| async move {
        c.get_guild_stats(TonicRequest::new(GuildStatsRequest { guild_id })).await
    })
    .await?
    .into_inner();
    Ok(Json(GuildStatsResponse {
        guild_id: resp.guild_id,
        messages: resp.messages,
        commands: resp.commands,
    }))
}

pub async fn top_commands(State(state): State<AppState>) -> Result<Json<TopCommandsResponse>, ApiError> {
    let resp = rpc_call(&state, |mut c| async move {
        c.get_top_commands(TonicRequest::new(TopCommandsRequest { limit: 15 })).await
    })
    .await?
    .into_inner();
    Ok(Json(TopCommandsResponse {
        commands: resp
            .commands
            .into_iter()
            .map(|u| TopCommandEntry { command: u.command, count: u.count })
            .collect(),
    }))
}

pub async fn list_commands(State(state): State<AppState>) -> Result<Json<Vec<CommandInfoDto>>, ApiError> {
    let resp = rpc_call(&state, |mut c| async move {
        c.list_commands(TonicRequest::new(Empty {})).await
    })
    .await?
    .into_inner();
    Ok(Json(
        resp.commands
            .into_iter()
            .map(|c| CommandInfoDto {
                name: c.name,
                description: c.description,
                aliases: c.aliases,
                category: c.category,
                slash: c.slash,
                prefix: c.prefix,
                guild_only: c.guild_only,
                owner_only: c.owner_only,
                is_script: c.is_script,
            })
            .collect(),
    ))
}

pub async fn list_triggers(State(state): State<AppState>) -> Result<Json<Vec<TriggerInfoDto>>, ApiError> {
    let resp = rpc_call(&state, |mut c| async move {
        c.list_triggers(TonicRequest::new(Empty {})).await
    })
    .await?
    .into_inner();
    Ok(Json(
        resp.triggers
            .into_iter()
            .map(|t| TriggerInfoDto {
                name: t.name,
                matcher_kind: t.matcher_kind,
                matcher_arg: t.matcher_arg,
                is_script: t.is_script,
            })
            .collect(),
    ))
}

pub async fn reload(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
) -> Result<Json<ReloadResponse>, ApiError> {
    let requester = session.user_id;
    let resp = rpc_call(&state, move |mut c| async move {
        c.reload_scripts(TonicRequest::new(ReloadRequest { requester_id: requester })).await
    })
    .await?
    .into_inner();
    Ok(Json(ReloadResponse {
        success: resp.success,
        message: resp.message,
        command_count: resp.command_count,
        trigger_count: resp.trigger_count,
    }))
}

// ---- gRPC plumbing ----

async fn rpc_call<F, Fut, T>(state: &AppState, f: F) -> Result<tonic::Response<T>, ApiError>
where
    F: FnOnce(crate::state::RpcClient) -> Fut,
    Fut: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
{
    let client = state.rpc.clone();
    f(client).await.map_err(|e| {
        warn!(code = ?e.code(), message = %e.message(), "rpc call failed");
        ApiError::Rpc(e)
    })
}

// ---- Errors ----

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("rpc: {0}")]
    Rpc(#[from] tonic::Status),
    #[error("internal: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            ApiError::Rpc(s) => (
                match s.code() {
                    tonic::Code::PermissionDenied => StatusCode::FORBIDDEN,
                    tonic::Code::Unauthenticated => StatusCode::UNAUTHORIZED,
                    tonic::Code::NotFound => StatusCode::NOT_FOUND,
                    _ => StatusCode::BAD_GATEWAY,
                },
                s.message().to_string(),
            ),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (
            status,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response()
    }
}
