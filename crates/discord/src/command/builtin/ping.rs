use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct PingCommand;

#[async_trait]
impl Command for PingCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("ping", "Check that the bot is alive").category("General")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let latency = ctx.started_at.elapsed();
        ctx.reply_embed(&Embed2::success().title("Pong!").description(format!("Round trip: `{:?}`", latency))).await
    }
}
