//! `nhentai <id-or-url>` — fetch and display a gallery from nhentai.net.
//! NSFW-channel-only. Scrapes the public HTML page since the JSON API is
//! gone.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;
use tomo_requester::nhentai::parse_gallery_id;

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
use crate::util::truncate;

pub struct NhentaiCommand;

#[async_trait]
impl Command for NhentaiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("nhentai", "Look up a gallery by id or URL.")
                .aliases(["nhen", "nh"])
                .category("Search")
                .guild_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        if !is_nsfw_channel(&ctx) {
            return ctx.reply("That command is NSFW-only.").await;
        }

        let Some(id) = extract_id(&ctx) else {
            return ctx
                .reply("Give me a gallery id or an `nhentai.net/g/<id>/` URL.")
                .await;
        };

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let gallery = match ctx.bot.requester.nhentai(id).await {
            Ok(g) => g,
            Err(e) => return ctx.reply(&format!("nhentai lookup failed: `{e}`")).await,
        };

        let mut embed = Embed2::lovely()
            .title(truncate(gallery.best_title(), 256))
            .url(gallery.page_url());

        if let Some(cover) = gallery.cover_url.as_ref() {
            embed = embed.image(cover.clone());
        }
        if let Some(n) = gallery.page_count {
            embed = embed.field_inline("Pages", n.to_string());
        }
        if let Some(n) = gallery.favorites {
            embed = embed.field_inline("Favs", n.to_string());
        }
        if let Some(jpn) = gallery.title_japanese.as_ref() {
            embed = embed.field_block("Japanese", truncate(jpn, 1024));
        }

        let artists: Vec<_> = gallery.tags_of("artist").collect();
        if !artists.is_empty() {
            embed = embed.field_block("Artist", truncate(&artists.join(", "), 1024));
        }
        let parodies: Vec<_> = gallery.tags_of("parody").collect();
        if !parodies.is_empty() {
            embed = embed.field_block("Parody", truncate(&parodies.join(", "), 1024));
        }
        let langs: Vec<_> = gallery.tags_of("language").collect();
        if !langs.is_empty() {
            embed = embed.field_inline("Language", langs.join(", "));
        }
        let tags: Vec<_> = gallery.tags_of("tag").take(30).collect();
        if !tags.is_empty() {
            embed = embed.field_block("Tags", truncate(&tags.join(", "), 1024));
        }
        if let Some(when) = gallery.upload_unix {
            embed = embed.timestamp_unix(when);
        }

        ctx.reply_embed(&embed).await
    }
}

fn extract_id(ctx: &CommandContext) -> Option<u64> {
    let args = ctx.args().trim();
    if let Some(id) = parse_gallery_id(args) {
        return Some(id);
    }
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if let Some(referenced) = msg.referenced_message.as_deref() {
            if let Some(id) = parse_gallery_id(&referenced.content) {
                return Some(id);
            }
        }
    }
    None
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}
