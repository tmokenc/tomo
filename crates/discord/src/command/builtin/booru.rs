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
            .string_option(
                "query",
                "Optional source (yandere/konachan/danbooru) + tags, e.g. `yandere catgirl 2girls`",
                true,
            )
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let raw = ctx.args().trim();
        if raw.is_empty() {
            return ctx
                .reply_embed(
                    &Embed2::warning()
                        .title("Booru")
                        .description("Usage: `booru [yandere|konachan|danbooru] <tags>`")
                        .timestamp_now(),
                )
                .await;
        }

        let mut tokens = raw.split_whitespace();
        let first = tokens.next().unwrap_or("");
        let (source, tags): (BooruSource, Vec<&str>) = match BooruSource::parse(first) {
            Some(s) => (s, tokens.collect()),
            None => (BooruSource::Yandere, raw.split_whitespace().collect()),
        };

        if tags.is_empty() {
            return ctx
                .reply_embed(
                    &Embed2::warning()
                        .title(source.name())
                        .description("Please give me at least one tag to search for.")
                        .timestamp_now(),
                )
                .await;
        }

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        let posts = match ctx.bot.requester.booru(source, &tags, 20).await {
            Ok(p) => p,
            Err(e) => {
                return reply_error(&ctx, source, &format!("Booru request failed: `{e}`")).await
            }
        };

        let post = {
            let mut rng = rand::rng();
            posts.choose(&mut rng).cloned()
        };

        let Some(post) = post else {
            return reply_error(&ctx, source, "No results for those tags.").await;
        };

        // Block NSFW in non-NSFW channels — Discord doesn't surface that flag
        // through twilight conveniently, so we err on the side of caution and
        // refuse explicit content unconditionally for now.
        if matches!(post.rating, BooruRating::Explicit) {
            return reply_error(
                &ctx,
                source,
                "That tag returned explicit-rated results; refusing.",
            )
            .await;
        }

        let mut embed = Embed2::info()
            .title(format!("{} #{}", source.name(), post.id))
            .url(source.post_url(post.id))
            .description(truncate(&post.tags.replace(' ', ", "), 4000))
            .image(post.file_url.clone())
            .field_inline("Rating", post.rating.label())
            .field_inline("Score", post.score.to_string())
            .field_inline("Size", format!("{}×{}", post.width, post.height))
            .footer_with(source.name(), source_icon(source))
            .timestamp_now();

        if !post.author.is_empty() {
            embed = embed.field_inline("Author", truncate(&post.author, 256));
        }
        if let Some(src) = post.source_url.as_ref().filter(|s| !s.is_empty()) {
            embed = embed.field_block("Source", truncate(src, 1024));
        }
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}

fn source_icon(source: BooruSource) -> &'static str {
    match source {
        // Tiny site favicons — handy for the footer to make it scannable.
        BooruSource::Yandere => "https://yande.re/favicon.ico",
        BooruSource::Konachan => "https://konachan.com/favicon.ico",
        BooruSource::Danbooru => "https://danbooru.donmai.us/favicon.ico",
    }
}

async fn reply_error(ctx: &CommandContext, source: BooruSource, message: &str) -> Result<()> {
    let mut embed = Embed2::error()
        .title(source.name())
        .description(message.to_string())
        .footer_with(source.name(), source_icon(source))
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}
