use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::runtime;

pub struct ReloadCommand;

#[async_trait]
impl Command for ReloadCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("reload", "Reload Rhai scripts from disk")
                .category("Admin")
                .owner_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        ctx.bot.scripts.reload().await?;
        runtime::refresh_command_registry(&ctx.bot);
        runtime::refresh_trigger_registry(&ctx.bot);
        let snap = ctx.bot.scripts.registry();
        ctx.reply_embed(
            &Embed2::success()
                .title("Scripts reloaded")
                .field("Commands", snap.commands.len().to_string(), true)
                .field("Triggers", snap.triggers.len().to_string(), true),
        )
        .await
    }
}
