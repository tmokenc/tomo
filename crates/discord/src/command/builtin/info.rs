use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use humantime::format_duration;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct InfoCommand;

#[async_trait]
impl Command for InfoCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("info", "Show information about the bot")
                .aliases(["information", "about"])
                .category("General")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let uptime: Duration = ctx.bot.uptime().to_std().unwrap_or_default();
        let global = ctx.bot.stats.global().await?;
        let id = &ctx.bot.identity;
        let started = ctx.bot.started_at;

        let description = if id.application_description.is_empty() {
            "A multi-service Rust bot. Slash & prefix commands, Rhai scripting, \
             auto-triggers, Gemini, gRPC control plane, web admin."
                .to_string()
        } else {
            id.application_description.clone()
        };

        let mut embed = Embed2::info()
            .title(format!("{} — Info", id.username))
            .description(description)
            .author_with(id.full_username(), id.avatar_url.clone(), None::<String>)
            .field_inline("Uptime", format_duration(uptime).to_string())
            .field_inline("Commands run", global.commands.to_string())
            .field_inline("Owners", ctx.bot.owners.len().to_string())
            .field_inline("Bot user", id.full_username())
            .field_inline("User id", id.user_id.to_string())
            .field_inline("Application id", id.application_id.to_string())
            .field_inline("Public", if id.bot_public { "yes" } else { "no" })
            .field_inline("Verified", if id.verified { "yes" } else { "no" })
            .field_inline("Messages seen", global.messages.to_string());

        if let Some(owner) = id.owner.as_ref() {
            embed = embed.field_inline("Owner", format!("{} (`{}`)", owner.username, owner.id));
        }
        embed = embed.field_block(
            "Started",
            format!("<t:{0}:F> (<t:{0}:R>)", started.timestamp()),
        );

        if let Some(url) = id.avatar_url.as_ref() {
            embed = embed.thumbnail(url.clone());
        }
        embed = embed
            .timestamp(started)
            .footer(format!("Up since {}", started.format("%Y-%m-%d %H:%M UTC")));

        ctx.reply_embed(&embed).await
    }
}
