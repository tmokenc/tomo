//! Boot the admin HTTP server.

use std::time::Duration;

use async_trait::async_trait;
use axum::middleware::{from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tonic::transport::Endpoint;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

use tomo_core::{Error, Result as CoreResult, Service};
use tomo_rpc::auth::BearerInterceptor;
use tomo_rpc::TomoControlClient;

use crate::config::AdminConfig;
use crate::state::AppState;
use crate::{api, middleware, oauth, security, static_files};

pub struct AdminService {
    state: AppState,
}

impl AdminService {
    /// Open a gRPC connection to the bot and build the [`AppState`].
    pub async fn bootstrap(config: AdminConfig) -> CoreResult<Self> {
        info!(rpc = %config.rpc_url, bind = %config.bind, "admin bootstrap");

        let endpoint = Endpoint::from_shared(config.rpc_url.clone())
            .map_err(|e| Error::config(format!("rpc endpoint: {e}")))?
            .timeout(Duration::from_secs(10))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .connect_timeout(Duration::from_secs(5));

        // Connect lazily so the admin service doesn't fail bootstrap if the
        // bot's gRPC server hasn't come up yet — first request will retry.
        let channel = endpoint.connect_lazy();

        let interceptor = BearerInterceptor::new(config.rpc_token.clone());
        let client = TomoControlClient::with_interceptor(channel, interceptor);

        Ok(Self { state: AppState::new(config, client) })
    }
}

#[async_trait]
impl Service for AdminService {
    fn name(&self) -> &'static str { "admin" }

    async fn run(self: Box<Self>, mut shutdown: watch::Receiver<bool>) -> CoreResult<()> {
        let state = self.state;
        let dist = state.config.frontend_dist.clone();
        let bind = state.config.bind;

        // API routes — owner-only via middleware.
        let api_router = Router::new()
            .route("/me", get(api::me))
            .route("/status", get(api::status))
            .route("/stats/global", get(api::global_stats))
            .route("/stats/user/{user_id}", get(api::user_stats))
            .route("/stats/guild/{guild_id}", get(api::guild_stats))
            .route("/stats/top", get(api::top_commands))
            .route("/commands", get(api::list_commands))
            .route("/triggers", get(api::list_triggers))
            .route("/reload", post(api::reload))
            .route_layer(from_fn_with_state(state.clone(), middleware::require_owner));

        let app = Router::new()
            .route("/login", get(oauth::login))
            .route("/logout", get(oauth::logout))
            .route("/oauth/callback", get(oauth::callback))
            .nest("/api", api_router)
            .fallback_service(static_files::router(dist))
            .layer(from_fn_with_state(state.clone(), security::apply_headers))
            .layer(TraceLayer::new_for_http())
            .with_state(state);

        let listener = TcpListener::bind(bind)
            .await
            .map_err(|e| Error::config(format!("bind {bind}: {e}")))?;

        let shutdown_signal = async move {
            let _ = shutdown.changed().await;
            info!("admin server shutting down");
        };

        info!(addr = %bind, "admin server listening");
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await
        {
            warn!(error = %e, "admin server exited");
        }
        Ok(())
    }
}
