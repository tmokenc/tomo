//! gRPC server side of `tomo-rpc`. Implements [`TomoControl`] backed by
//! [`BotState`] and runs as a [`Service`] so the main binary can launch it
//! alongside the gateway.

use std::net::SocketAddr;

use async_trait::async_trait;
use tokio::sync::watch;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use tomo_core::{Error, Result as CoreResult, Service};
use tomo_rpc::auth::TokenInterceptor;
use tomo_rpc::proto::*;

use crate::state::Bot;

/// Configuration for the gRPC service.
#[derive(Debug, Clone)]
pub struct RpcConfig {
    /// Where to bind. Defaults to `127.0.0.1:50051` — externally
    /// inaccessible. Operators who want remote access should put a reverse
    /// proxy with TLS in front; do not bind `0.0.0.0` without one.
    pub bind: SocketAddr,
    /// Optional bearer token required on every inbound request.
    pub token: Option<String>,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:50051".parse().expect("static valid SocketAddr"),
            token: None,
        }
    }
}

pub struct RpcService {
    bot: Bot,
    cfg: RpcConfig,
}

impl RpcService {
    pub fn new(bot: Bot, cfg: RpcConfig) -> Self {
        Self { bot, cfg }
    }
}

#[async_trait]
impl Service for RpcService {
    fn name(&self) -> &'static str { "rpc" }

    async fn run(self: Box<Self>, mut shutdown: watch::Receiver<bool>) -> CoreResult<()> {
        let RpcService { bot, cfg } = *self;
        let svc = TomoControlImpl { bot };
        let interceptor = TokenInterceptor::new(cfg.token.clone());

        let server = tomo_rpc::TomoControlServer::with_interceptor(svc, move |req| {
            interceptor.intercept(req)
        });

        info!(addr = %cfg.bind, auth = cfg.token.is_some(), "gRPC server listening");

        let shutdown_signal = async move {
            let _ = shutdown.changed().await;
            info!("gRPC server shutting down");
        };

        Server::builder()
            .add_service(server)
            .serve_with_shutdown(cfg.bind, shutdown_signal)
            .await
            .map_err(|e| Error::config(format!("gRPC server: {e}")))?;

        Ok(())
    }
}

// ---------- service impl ----------

struct TomoControlImpl {
    bot: Bot,
}

#[tonic::async_trait]
impl tomo_rpc::TomoControl for TomoControlImpl {
    async fn get_status(&self, _: Request<Empty>) -> Result<Response<BotStatus>, Status> {
        let uptime = self.bot.uptime().num_seconds().max(0);
        let id = &self.bot.identity;
        Ok(Response::new(BotStatus {
            name: id.username.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_unix: self.bot.started_at.timestamp(),
            uptime_seconds: uptime,
            owners: self.bot.owners.iter().map(|o| o.get()).collect(),
            bot_user_id: id.user_id.get(),
            gemini_enabled: self.bot.llm.is_some(),

            discriminator: id.discriminator as u32,
            avatar_url: id.avatar_url.clone().unwrap_or_default(),
            application_id: id.application_id.get(),
            application_name: id.application_name.clone(),
            application_description: id.application_description.clone(),
            bot_public: id.bot_public,
            bot_require_code_grant: id.bot_require_code_grant,
            verified: id.verified,
            owner_id: id.owner.as_ref().map(|o| o.id.get()).unwrap_or(0),
            owner_username: id.owner.as_ref().map(|o| o.username.clone()).unwrap_or_default(),
        }))
    }

    async fn get_global_stats(&self, _: Request<Empty>) -> Result<Response<GlobalStats>, Status> {
        let g = self.bot.stats.global().await.map_err(stats_err)?;
        Ok(Response::new(GlobalStats {
            messages: g.messages,
            commands: g.commands,
            gemini_calls: g.gemini_calls,
            gemini_tokens: g.gemini_tokens,
        }))
    }

    async fn get_user_stats(&self, req: Request<UserStatsRequest>) -> Result<Response<UserStats>, Status> {
        let user_id = req.into_inner().user_id;
        let id = twilight_model::id::Id::new(user_id.max(1));
        let s = self.bot.stats.user_stats(id).await.map_err(stats_err)?;
        Ok(Response::new(UserStats {
            user_id: s.user_id,
            messages: s.messages,
            commands: s.commands,
            gemini_calls: s.gemini_calls,
            gemini_tokens: s.gemini_tokens,
        }))
    }

    async fn get_guild_stats(&self, req: Request<GuildStatsRequest>) -> Result<Response<GuildStats>, Status> {
        let gid = req.into_inner().guild_id;
        let id = twilight_model::id::Id::new(gid.max(1));
        let s = self.bot.stats.guild_stats(id).await.map_err(stats_err)?;
        Ok(Response::new(GuildStats {
            guild_id: s.guild_id,
            messages: s.messages,
            commands: s.commands,
        }))
    }

    async fn get_top_commands(&self, req: Request<TopCommandsRequest>) -> Result<Response<TopCommandsResponse>, Status> {
        let limit = req.into_inner().limit.clamp(1, 100) as usize;
        let top = self.bot.stats.top_commands(limit).await.map_err(stats_err)?;
        Ok(Response::new(TopCommandsResponse {
            commands: top
                .into_iter()
                .map(|u| CommandUsage { command: u.command, count: u.count })
                .collect(),
        }))
    }

    async fn list_commands(&self, _: Request<Empty>) -> Result<Response<CommandList>, Status> {
        let registry = self.bot.commands.load();
        let scripts = self.bot.scripts.registry();
        let script_names: std::collections::HashSet<&str> =
            scripts.commands.values().map(|c| c.name.as_str()).collect();

        let mut commands = Vec::new();
        for cmd in registry.iter_unique() {
            let m = cmd.meta();
            commands.push(CommandInfo {
                name: m.name.clone(),
                description: m.description.clone(),
                aliases: m.aliases.clone(),
                category: m.category.to_string(),
                slash: m.slash,
                prefix: m.prefix,
                guild_only: m.guild_only,
                owner_only: m.owner_only,
                is_script: script_names.contains(m.name.as_str()),
            });
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Response::new(CommandList { commands }))
    }

    async fn list_triggers(&self, _: Request<Empty>) -> Result<Response<TriggerList>, Status> {
        let registry = self.bot.triggers.load();
        let mut triggers = Vec::new();

        for builtin in registry.iter_builtin() {
            triggers.push(TriggerInfo {
                name: builtin.name().to_string(),
                matcher_kind: "rust".into(),
                matcher_arg: String::new(),
                is_script: false,
            });
        }
        for script in registry.iter_script() {
            let (kind, arg) = describe_matcher(&script.matcher);
            triggers.push(TriggerInfo {
                name: script.name.clone(),
                matcher_kind: kind.into(),
                matcher_arg: arg,
                is_script: true,
            });
        }
        Ok(Response::new(TriggerList { triggers }))
    }

    async fn reload_scripts(&self, req: Request<ReloadRequest>) -> Result<Response<ReloadResult>, Status> {
        let requester = req.into_inner().requester_id;
        let id = twilight_model::id::Id::new(requester.max(1));
        if !self.bot.is_owner(id) {
            warn!(user = requester, "non-owner attempted ReloadScripts");
            return Err(Status::permission_denied("owner only"));
        }
        match self.bot.scripts.reload().await {
            Ok(()) => {
                crate::runtime::refresh_command_registry(&self.bot);
                crate::runtime::refresh_trigger_registry(&self.bot);
                let snap = self.bot.scripts.registry();
                Ok(Response::new(ReloadResult {
                    success: true,
                    message: "ok".into(),
                    command_count: snap.commands.len() as u32,
                    trigger_count: snap.triggers.len() as u32,
                }))
            }
            Err(e) => Ok(Response::new(ReloadResult {
                success: false,
                message: e.to_string(),
                command_count: 0,
                trigger_count: 0,
            })),
        }
    }

    async fn check_ownership(&self, req: Request<CheckOwnerRequest>) -> Result<Response<CheckOwnerResponse>, Status> {
        let user_id = req.into_inner().user_id;
        let id = twilight_model::id::Id::new(user_id.max(1));
        Ok(Response::new(CheckOwnerResponse { is_owner: self.bot.is_owner(id) }))
    }
}

fn describe_matcher(m: &tomo_scripting::TriggerMatcher) -> (&'static str, String) {
    use tomo_scripting::TriggerMatcher as M;
    match m {
        M::Regex(re) => ("regex", re.as_str().to_string()),
        M::HasImage => ("has_image", String::new()),
        M::HasAttachment => ("has_attachment", String::new()),
        M::Contains(s) => ("contains", s.clone()),
        M::StartsWith(s) => ("starts_with", s.clone()),
        M::MentionsBot => ("mentions_bot", String::new()),
    }
}

fn stats_err(e: tomo_core::Error) -> Status {
    Status::internal(format!("stats: {e}"))
}
