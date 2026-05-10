//! `ehentai <urls>` — look up one or more gallery URLs on E-Hentai's
//! `gdata` JSON-RPC endpoint. NSFW-channel-only; up to 25 galleries per
//! call (the upstream cap).

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;
use tomo_requester::ehentai::{parse_ids, EhentaiGallery};

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
use crate::util::truncate;

pub struct EhentaiCommand;

#[async_trait]
impl Command for EhentaiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "ehentai",
                "Show gallery info for one or more E-Hentai URLs.",
            )
            .aliases(["eh", "e-hentai", "sadkaede", "sadpanda"])
            .category("Search")
            .guild_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        if !is_nsfw_channel(&ctx) {
            return ctx.reply("That command is NSFW-only.").await;
        }

        let ids = gather_ids(&ctx);
        if ids.is_empty() {
            return ctx
                .reply("Give me one or more `e-hentai.org/g/<gid>/<token>/` URLs.")
                .await;
        }

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let galleries = match ctx.bot.requester.ehentai_gmetadata(&ids).await {
            Ok(g) => g,
            Err(e) => return ctx.reply(&format!("E-Hentai lookup failed: `{e}`")).await,
        };
        if galleries.is_empty() {
            return ctx.reply("Nothing came back.").await;
        }

        // For >1 result, render a compact list. For one, render rich details.
        if galleries.len() == 1 {
            let g = &galleries[0];
            ctx.reply_embed(&single_embed(g)).await
        } else {
            let mut body = String::new();
            for g in &galleries {
                let title = g.title.as_deref().or(g.title_jpn.as_deref()).unwrap_or("(no title)");
                body.push_str(&format!("• [{title}]({})\n", g.url()));
                if body.len() > 3500 {
                    body.push_str("…\n");
                    break;
                }
            }
            let embed = Embed2::lovely()
                .title(format!("{} galleries", galleries.len()))
                .description(truncate(&body, 4000));
            ctx.reply_embed(&embed).await
        }
    }
}

fn single_embed(g: &EhentaiGallery) -> Embed2 {
    let title = g.title.as_deref().or(g.title_jpn.as_deref()).unwrap_or("(no title)");
    let mut e = Embed2::lovely()
        .title(truncate(title, 256))
        .url(g.url())
        .thumbnail(g.thumb.clone())
        .field_inline("Category", g.category.clone())
        .field_inline("Pages", g.filecount.clone())
        .field_inline("Rating", g.rating.clone())
        .field_inline("Uploader", g.uploader.clone());
    if let Some(jpn) = &g.title_jpn {
        if jpn != title {
            e = e.field_block("Japanese title", truncate(jpn, 1024));
        }
    }
    if !g.tags.is_empty() {
        let tags_str = g.tags.iter().take(30).cloned().collect::<Vec<_>>().join(", ");
        e = e.field_block("Tags", truncate(&tags_str, 1024));
    }
    if let Ok(ts) = g.posted.parse::<i64>() {
        e = e.timestamp_unix(ts);
    }
    e
}

fn gather_ids(ctx: &CommandContext) -> Vec<(u64, String)> {
    let mut ids = parse_ids(ctx.args());
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if let Some(referenced) = msg.referenced_message.as_deref() {
            ids.extend(parse_ids(&referenced.content));
        }
    }
    // Dedup.
    ids.sort();
    ids.dedup();
    ids
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}
