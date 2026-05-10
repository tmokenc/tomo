//! `booru [source] <tags...>` — fetch a random post from a booru.

use std::sync::LazyLock;

use async_trait::async_trait;
use rand::seq::IndexedRandom;

use tomo_core::error::Result;
use tomo_embed::Embed2;
use tomo_requester::{BooruRating, BooruSource};

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::truncate;

pub struct BooruCommand;

#[async_trait]
impl Command for BooruCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "booru",
                "Fetch a random image from a booru (yandere/konachan/danbooru)",
            )
            .category("Search")
            .guild_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let raw = ctx.args().trim();
        if raw.is_empty() {
            return ctx
                .reply("Usage: `booru [yandere|konachan|danbooru] <tags>`")
                .await;
        }

        let mut tokens = raw.split_whitespace();
        let first = tokens.next().unwrap_or("");
        let (source, tags): (BooruSource, Vec<&str>) = match BooruSource::parse(first) {
            Some(s) => (s, tokens.collect()),
            None => (BooruSource::Yandere, raw.split_whitespace().collect()),
        };

        if tags.is_empty() {
            return ctx.reply("Please give me at least one tag.").await;
        }

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        let posts = match ctx.bot.requester.booru(source, &tags, 20).await {
            Ok(p) => p,
            Err(e) => {
                return ctx
                    .reply_embed(
                        &Embed2::error()
                            .title(source.name())
                            .description(format!("Booru request failed: `{e}`")),
                    )
                    .await
            }
        };

        let post = {
            let mut rng = rand::rng();
            posts.choose(&mut rng).cloned()
        };

        let Some(post) = post else {
            return ctx
                .reply_embed(
                    &Embed2::error()
                        .title(source.name())
                        .description("No results for those tags."),
                )
                .await;
        };

        // Block NSFW in non-NSFW channels — Discord doesn't surface that flag
        // through twilight conveniently, so we err on the side of caution and
        // refuse explicit content unconditionally for now.
        if matches!(post.rating, BooruRating::Explicit) {
            return ctx
                .reply("That tag returned explicit-rated results; refusing.")
                .await;
        }

        let embed = Embed2::info()
            .title(format!("{} #{}", source.name(), post.id))
            .description(truncate(&post.tags.replace(' ', ", "), 1024))
            .image(post.file_url.clone())
            .footer(format!("score {} · {}×{}", post.score, post.width, post.height));
        ctx.reply_embed(&embed).await
    }
}

