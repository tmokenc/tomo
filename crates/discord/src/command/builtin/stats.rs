use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct StatsCommand;

#[async_trait]
impl Command for StatsCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> =
            LazyLock::new(|| CommandMeta::new("stats", "Show global bot statistics").category("Stats"));
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let g = ctx.bot.stats.global().await?;
        let me = ctx.bot.stats.user_stats(ctx.author_id()).await?;
        let mut embed = Embed2::info()
            .title("Statistics")
            .description("Numbers since the database was first created.")
            .field_inline("Messages seen", g.messages.to_string())
            .field_inline("Commands run", g.commands.to_string())
            .field_inline("Gemini calls", g.gemini_calls.to_string())
            .field_inline("Gemini tokens", g.gemini_tokens.to_string())
            .field_block("\u{200b}", "**— Your stats —**")
            .field_inline("Your messages", me.messages.to_string())
            .field_inline("Your commands", me.commands.to_string())
            .field_inline("Your Gemini calls", me.gemini_calls.to_string());

        if let Some(guild_id) = ctx.guild_id() {
            let gs = ctx.bot.stats.guild_stats(guild_id).await?;
            let guild_name = ctx
                .bot
                .cache
                .guild(guild_id)
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "this server".to_string());
            embed = embed
                .field_block("\u{200b}", format!("**— {guild_name} —**"))
                .field_inline("Server messages", gs.messages.to_string())
                .field_inline("Server commands", gs.commands.to_string());
        }

        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        if let Some(avatar) = ctx.author_avatar_url() {
            embed = embed.thumbnail(avatar);
        }
        embed = embed
            .footer(format!("Bot started {}", ctx.bot.started_at.format("%Y-%m-%d %H:%M UTC")))
            .timestamp_now();

        ctx.reply_embed(&embed).await
    }
}
