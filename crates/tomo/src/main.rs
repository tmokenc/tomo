//! Entry point. Loads `.env`, sets up tracing, opens the database, and runs
//! every enabled service concurrently.
//!
//! Services launched today:
//! * [`tomo_discord::DiscordService`] — gateway, commands, triggers, LLM router.
//! * [`tomo_discord::RpcService`]    — gRPC control plane.
//! * [`tomo_admin::AdminService`]    — axum-based web UI for owners.
//!
//! New services (telegram bot, etc.) just need to implement
//! [`tomo_core::Service`] and get pushed onto the `services` vec.

use std::env;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use tokio::signal;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use tomo_admin::{AdminConfig, AdminService};
use tomo_core::{Config, Service};
use tomo_db::FjallStore;
use tomo_discord::{DiscordService, RpcConfig, RpcService};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // Load `.env` *before* tracing init so `RUST_LOG` set in `.env` is
    // visible to `EnvFilter::try_from_default_env`. Without this the
    // subscriber initialises before dotenvy has populated env vars and
    // falls back to its hard-coded default; the `.env`-supplied
    // `RUST_LOG` wouldn't apply until later code that's invisible (no
    // tracing yet to observe it).
    let dotenv_outcome = dotenvy::dotenv();

    init_tracing();

    match &dotenv_outcome {
        Ok(path) => info!(path = %path.display(), "loaded .env"),
        Err(e) if e.not_found() => warn!(
            cwd = ?env::current_dir().ok(),
            "no .env file found — relying on process environment only"
        ),
        Err(e) => warn!(error = %e, "failed to load .env"),
    }

    let config = Arc::new(Config::from_env().context("loading configuration")?);
    info!(
        prefix = %config.discord.prefix,
        data_dir = ?config.data_dir,
        script_dir = ?config.script_dir,
        llm = config.llm.is_some(),
        "tomo starting"
    );

    let store = FjallStore::open(&config.data_dir)
        .await
        .context("opening database")?;
    let store: Arc<dyn tomo_db::KvStore> = Arc::new(store);

    let discord = DiscordService::bootstrap(Arc::clone(&config), Arc::clone(&store))
        .await
        .context("discord bootstrap")?;
    let bot = discord.bot();

    let mut services: Vec<Box<dyn Service>> = Vec::new();
    services.push(Box::new(discord));

    // gRPC server — the admin web UI reaches the bot exclusively over this
    // gRPC surface, so force it on when admin is enabled (otherwise the admin
    // service starts but can never talk to the bot). Also useful standalone
    // for ad-hoc tooling (grpcurl, etc.).
    if rpc_enabled() || AdminConfig::enabled() {
        if !rpc_enabled() {
            info!("admin service enabled — starting RPC server it depends on");
        }
        let rpc_cfg = rpc_config_from_env()?;
        info!(bind = %rpc_cfg.bind, "starting RPC service");
        services.push(Box::new(RpcService::new(bot.clone(), rpc_cfg)));
    }

    // Admin web UI.
    if AdminConfig::enabled() {
        match AdminConfig::from_env() {
            Ok(cfg) => match AdminService::bootstrap(cfg).await {
                Ok(svc) => services.push(Box::new(svc)),
                Err(e) => warn!(error = %e, "admin bootstrap failed; skipping"),
            },
            Err(e) => warn!(error = %e, "admin config invalid; skipping"),
        }
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn every service into a JoinSet so they can be *supervised*. A
    // service's `run` is expected to block until shutdown; if one returns
    // early — cleanly or with an error — the bot is degraded (e.g. the gateway
    // got a fatal close like 4014 "disallowed intents", or the RPC listener
    // failed to bind). Leaving the process up in that state is the zombie bug:
    // no gateway, but a live RPC/admin surface still reporting healthy. So an
    // early exit tears the whole process down, exactly as a signal would.
    let mut set: JoinSet<&'static str> = JoinSet::new();
    for svc in services {
        let name = svc.name();
        let rx = shutdown_rx.clone();
        set.spawn(async move {
            match svc.run(rx).await {
                Ok(()) => info!(service = name, "service exited cleanly"),
                Err(e) => error!(service = name, error = %e, "service exited with error"),
            }
            name
        });
    }

    tokio::select! {
        _ = wait_for_signal() => {
            info!("shutdown signal received");
        }
        joined = set.join_next() => match joined {
            Some(Ok(name)) => {
                warn!(service = name, "service exited before shutdown — stopping the bot");
            }
            Some(Err(e)) => {
                error!(error = %e, "a service task panicked — stopping the bot");
            }
            None => warn!("no services were running — nothing to do"),
        },
    }

    let _ = shutdown_tx.send(true);

    // Drain the remaining services, bounded so a wedged one can't hang exit.
    let drain = async {
        while let Some(joined) = set.join_next().await {
            if let Err(e) = joined {
                warn!(error = %e, "service task join error during shutdown");
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(10), drain).await.is_err() {
        warn!("services did not all stop within 10s — exiting anyway");
    }

    info!("tomo shut down");
    Ok(())
}

fn rpc_enabled() -> bool {
    env::var("TOMO_ENABLE_RPC")
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn rpc_config_from_env() -> anyhow::Result<RpcConfig> {
    let bind: SocketAddr = env::var("TOMO_RPC_BIND")
        .ok()
        .as_deref()
        .map(SocketAddr::from_str)
        .transpose()
        .context("TOMO_RPC_BIND")?
        .unwrap_or_else(|| "127.0.0.1:50051".parse().unwrap());
    let token = env::var("TOMO_RPC_TOKEN").ok().filter(|s| !s.trim().is_empty());
    Ok(RpcConfig { bind, token })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tomo=debug,twilight_gateway=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "cannot install SIGTERM handler");
                let _ = signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}
