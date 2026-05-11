//! `urban` — search Urban Dictionary.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::truncate;

/// Public-domain Urban Dictionary "UD" logo. Used as a footer icon so the
/// definition embed has a recognisable mark without bloating the layout.
const UD_ICON: &str = "https://www.urbandictionary.com/static/icon-180x180.png";

pub struct UrbanCommand;

#[async_trait]
impl Command for UrbanCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("urban", "Look up a slang term on Urban Dictionary")
                .aliases(["u", "ud"])
                .category("Search")
                .string_option("term", "Word or phrase to look up (empty = random)", false)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let term = ctx.args().trim();
        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        let results = if term.is_empty() {
            ctx.bot.requester.urban_random().await
        } else {
            ctx.bot.requester.urban_search(term).await
        };

        let definitions = match results {
            Ok(d) => d,
            Err(e) => {
                let mut err = Embed2::error()
                    .title("Urban Dictionary")
                    .description(format!("Lookup failed: `{e}`"))
                    .footer_with("urbandictionary.com", UD_ICON)
                    .timestamp_now();
                if let Some(user) = ctx.author() {
                    err = err.author_user(user);
                }
                return ctx.reply_embed(&err).await;
            }
        };

        let Some(def) = definitions.into_iter().next() else {
            let mut empty = Embed2::warning()
                .title(format!("Definition of `{term}`"))
                .description("No results.")
                .footer_with("urbandictionary.com", UD_ICON)
                .timestamp_now();
            if let Some(user) = ctx.author() {
                empty = empty.author_user(user);
            }
            return ctx.reply_embed(&empty).await;
        };

        let mut embed = Embed2::info()
            .title(def.word.clone())
            .description(truncate(&def.definition, 2000))
            .url(def.permalink.clone())
            .field_block("Example", truncate(&def.example, 1024))
            .field_inline("👍", def.thumbs_up.to_string())
            .field_inline("👎", def.thumbs_down.to_string())
            .field_inline(
                "Score",
                (def.thumbs_up as i64 - def.thumbs_down as i64).to_string(),
            )
            .author_with(format!("by {}", def.author), None::<String>, None::<String>)
            .footer_with("Urban Dictionary", UD_ICON)
            .timestamp_now();

        if let Some(user) = ctx.author() {
            // Re-author with invoker + put the definer in the footer instead,
            // so the embed clearly attributes the query and the definition.
            embed = embed
                .author_user(user)
                .footer_with(format!("Defined by {} · Urban Dictionary", def.author), UD_ICON);
        }
        ctx.reply_embed(&embed).await
    }
}
