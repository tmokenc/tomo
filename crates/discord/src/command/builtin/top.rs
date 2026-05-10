use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct TopCommand;

#[async_trait]
impl Command for TopCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> =
            LazyLock::new(|| CommandMeta::new("top", "Most-used commands").category("Stats"));
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let top = ctx.bot.stats.top_commands(10).await?;
        if top.is_empty() {
            return ctx.reply("No commands have been used yet.").await;
        }
        let mut body = String::new();
        for (i, row) in top.iter().enumerate() {
            body.push_str(&format!("`#{:>2}` **{}** — {}\n", i + 1, row.command, row.count));
        }
        ctx.reply_embed(&Embed2::info().title("Top commands").description(body)).await
    }
}
