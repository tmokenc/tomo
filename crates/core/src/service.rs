use async_trait::async_trait;
use tokio::sync::watch;

use crate::error::Result;

/// A long-running service the binary can launch — Discord bot, Telegram bot,
/// HTTP server, etc. Each service runs concurrently and listens to a shared
/// shutdown signal.
#[async_trait]
pub trait Service: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn run(self: Box<Self>, shutdown: watch::Receiver<bool>) -> Result<()>;
}
