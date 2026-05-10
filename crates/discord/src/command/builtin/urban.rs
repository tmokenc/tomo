//! `urban` — search Urban Dictionary.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::truncate;

pub struct UrbanCommand;

#[async_trait]
impl Command for UrbanCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("urban", "Look up a slang term on Urban Dictionary")
                .aliases(["u", "ud"])
                .category("Search")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let term = ctx.args().trim();
        let typing = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let _ = typing;

        let results = if term.is_empty() {
            ctx.bot.requester.urban_random().await
        } else {
            ctx.bot.requester.urban_search(term).await
        };

        let definitions = match results {
            Ok(d) => d,
            Err(e) => {
                return ctx
                    .reply_embed(
                        &Embed2::error()
                            .title("Urban Dictionary")
                            .description(format!("Lookup failed: `{e}`")),
                    )
                    .await
            }
        };

        let Some(def) = definitions.into_iter().next() else {
            return ctx
                .reply_embed(
                    &Embed2::error()
                        .title(format!("Definition of `{term}`"))
                        .description("404 Not Found"),
                )
                .await;
        };

        let definition = truncate(&def.definition, 2000);
        let example = truncate(&def.example, 1024);
        let embed = Embed2::info()
            .title(format!("Definition of {}", def.word))
            .description(definition)
            .url(def.permalink)
            .field("Example", example, false)
            .field("👍", def.thumbs_up.to_string(), true)
            .field("👎", def.thumbs_down.to_string(), true)
            .footer(format!("by {}", def.author));
        ctx.reply_embed(&embed).await
    }
}

