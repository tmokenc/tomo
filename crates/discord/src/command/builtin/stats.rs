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
            .field("Messages seen", g.messages.to_string(), true)
            .field("Commands run", g.commands.to_string(), true)
            .field("Gemini calls", g.gemini_calls.to_string(), true)
            .field("Gemini tokens", g.gemini_tokens.to_string(), true)
            .field("— Your stats —", "\u{200b}", false)
            .field("Your messages", me.messages.to_string(), true)
            .field("Your commands", me.commands.to_string(), true)
            .field("Your Gemini calls", me.gemini_calls.to_string(), true);
        if let Some(guild_id) = ctx.guild_id() {
            let gs = ctx.bot.stats.guild_stats(guild_id).await?;
            embed = embed
                .field("— This server —", "\u{200b}", false)
                .field("Server messages", gs.messages.to_string(), true)
                .field("Server commands", gs.commands.to_string(), true);
        }
        ctx.reply_embed(&embed).await
    }
}
