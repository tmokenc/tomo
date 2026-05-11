use std::sync::LazyLock;
use std::time::Instant;

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
        let start = Instant::now();
        ctx.bot.scripts.reload().await?;
        runtime::refresh_command_registry(&ctx.bot);
        runtime::refresh_trigger_registry(&ctx.bot);
        let elapsed = start.elapsed();

        let snap = ctx.bot.scripts.registry();
        let mut embed = Embed2::success()
            .title("Scripts reloaded")
            .description(format!(
                "Reloaded **{}** commands and **{}** triggers in `{:?}`.",
                snap.commands.len(),
                snap.triggers.len(),
                elapsed,
            ))
            .field_inline("Commands", snap.commands.len().to_string())
            .field_inline("Triggers", snap.triggers.len().to_string())
            .field_inline("Duration", format!("{elapsed:?}"))
            .footer(format!("Script dir: {}", ctx.bot.config.script_dir.display()))
            .timestamp_now();

        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}
