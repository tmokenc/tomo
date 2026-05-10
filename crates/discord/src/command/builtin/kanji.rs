//! `kanji <chars>` — look up the meanings/readings of one or more kanji.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::{Color, Embed2};

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::truncate;

pub struct KanjiCommand;

#[async_trait]
impl Command for KanjiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("kanji", "Look up the meaning of one or more kanji")
                .aliases(["k"])
                .category("Search")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let query = ctx.args().trim();
        if query.is_empty() {
            return ctx.reply("Please give me at least one kanji.").await;
        }

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let search = match ctx.bot.requester.kanji_search(query).await {
            Ok(s) => s,
            Err(e) => {
                return ctx
                    .reply_embed(
                        &Embed2::error().title("Kanji").description(format!("Lookup failed: `{e}`")),
                    )
                    .await
            }
        };

        if search.results.is_empty() {
            return ctx
                .reply_embed(&Embed2::error().title("Kanji").description("No kanji found."))
                .await;
        }

        let mut embed = Embed2::new()
            .color(Color::Custom(0x977df2))
            .title(format!("Kanji: {query}"));

        for kanji in search.results.iter().take(10) {
            let header = format!(
                "{} — {}{} | {}{}",
                kanji.kanji,
                kanji
                    .level
                    .map(|l| format!("(N{l}) "))
                    .unwrap_or_default(),
                kanji.mean,
                kanji.pretty_on(),
                kanji
                    .pretty_kun()
                    .map(|k| format!(" | {k}"))
                    .unwrap_or_default(),
            );
            let body = kanji.pretty_detail().unwrap_or_else(|| "—".to_string());
            embed = embed.field(header, truncate(&body, 1024), false);
        }

        ctx.reply_embed(&embed).await
    }
}

