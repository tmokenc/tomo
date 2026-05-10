use std::sync::Arc;

use axum_extra::extract::cookie::Key;
use sha2::{Digest, Sha512};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use tomo_rpc::auth::BearerInterceptor;
use tomo_rpc::TomoControlClient;

use crate::config::AdminConfig;

/// Concrete RPC client type — kept in one place so handlers all agree on it.
pub type RpcClient = TomoControlClient<InterceptedService<Channel, BearerInterceptor>>;

/// Shared application state for the admin web service.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AdminConfig>,
    pub cookie_key: Key,
    /// gRPC client — `Channel` is internally Arc-shared so cloning is cheap.
    pub rpc: RpcClient,
}

impl AppState {
    pub fn new(config: AdminConfig, rpc: RpcClient) -> Self {
        // `Key::from` needs >= 64 bytes; expand the user's 32+ byte secret
        // with SHA-512 so we don't pin operators to a specific length.
        let mut hasher = Sha512::new();
        hasher.update(b"tomo-admin-cookie-key:v1:");
        hasher.update(config.session_secret.as_bytes());
        let key_bytes = hasher.finalize();
        let cookie_key = Key::from(&key_bytes);
        Self {
            config: Arc::new(config),
            cookie_key,
            rpc,
        }
    }
}

// Make `Key` extractable from `AppState` so axum-extra's `SignedCookieJar`
// can pull it out automatically.
impl axum::extract::FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
