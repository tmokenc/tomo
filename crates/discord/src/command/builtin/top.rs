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
            return ctx
                .reply_embed(
                    &Embed2::warning()
                        .title("Top commands")
                        .description("No commands have been used yet.")
                        .timestamp_now(),
                )
                .await;
        }

        let total: u64 = top.iter().map(|r| r.count).sum();
        let mut body = String::with_capacity(top.len() * 48);
        for (i, row) in top.iter().enumerate() {
            let medal = match i {
                0 => "🥇",
                1 => "🥈",
                2 => "🥉",
                _ => "▫️",
            };
            let pct = if total > 0 {
                row.count as f64 * 100.0 / total as f64
            } else {
                0.0
            };
            body.push_str(&format!(
                "{medal} `#{:>2}` **{}** — `{}` ({:.1}%)\n",
                i + 1,
                row.command,
                row.count,
                pct,
            ));
        }

        let mut embed = Embed2::info()
            .title("Top commands")
            .description(body)
            .field_inline("Tracked", top.len().to_string())
            .field_inline("Total runs", total.to_string())
            .footer(format!("Showing top {} since stats first recorded", top.len()))
            .timestamp_now();
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}
