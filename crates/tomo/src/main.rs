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

use anyhow::Context as _;
use tokio::signal;
use tokio::sync::watch;
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

    // gRPC server — always enabled when the admin service is, and useful on
    // its own for ad-hoc tooling (grpcurl, etc.).
    if rpc_enabled() {
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

    let mut handles = Vec::new();
    for svc in services {
        let name = svc.name();
        let rx = shutdown_rx.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = svc.run(rx).await {
                error!(service = name, error = %e, "service exited with error");
            } else {
                info!(service = name, "service exited cleanly");
            }
        }));
    }

    wait_for_signal().await;
    info!("shutdown signal received");
    let _ = shutdown_tx.send(true);

    for handle in handles {
        let _ = handle.await;
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
