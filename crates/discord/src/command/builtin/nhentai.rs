//! `nhentai <id-or-url>` — fetch and display a gallery from nhentai.net.
//! NSFW-channel-only. Scrapes the public HTML page since the JSON API is
//! gone.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;
use tomo_requester::nhentai::parse_gallery_id;

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
use crate::nhentai_view::gallery_embed;
use crate::util::{cache_pick_from_human, message_only_links_to, suppress_embeds};

pub struct NhentaiCommand;

#[async_trait]
impl Command for NhentaiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("nhentai", "Look up a gallery by id or URL.")
                .aliases(["nhen", "nh"])
                .category("Search")
                .guild_only()
                .string_option("id", "Gallery id (e.g. 649114) or full nhentai.net URL", true)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        if !is_nsfw_channel(&ctx) {
            return reply_warn(&ctx, "That command is NSFW-only.").await;
        }

        let Some(id) = extract_id(&ctx) else {
            return reply_warn(
                &ctx,
                "Give me a gallery id or an `nhentai.net/g/<id>/` URL.",
            )
            .await;
        };

        // Slash: acknowledge immediately — the scrape can exceed Discord's 3s
        // initial-response window. Prefix: defer is a no-op.
        if ctx.is_slash() {
            let _ = ctx.defer().await;
        }
        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let gallery = match ctx.bot.requester.nhentai(id).await {
            Ok(g) => g,
            Err(e) => {
                let mut err = Embed2::error()
                    .title("nhentai")
                    .description(format!("Lookup failed: `{e}`"))
                    .timestamp_now();
                if let Some(user) = ctx.author() {
                    err = err.author_user(user);
                }
                return ctx.reply_embed(&err).await;
            }
        };

        // Routes to an interaction followup for slash (we deferred above) or a
        // normal reply for prefix — either way the invocation is acknowledged.
        ctx.reply_embed(&gallery_embed(&gallery, ctx.author())).await?;
        // If the user pasted an `nhentai.net/g/<id>/` URL with a prefix
        // invocation, hide Discord's auto-preview now that we've posted
        // our own embed.
        if let InvocationSource::Prefix { msg, .. } = &ctx.source {
            if message_only_links_to(&msg.content, &["nhentai.net"]) {
                suppress_embeds(&ctx.bot, msg.channel_id, msg.id).await;
            }
        }
        Ok(())
    }
}

async fn reply_warn(ctx: &CommandContext, message: &str) -> Result<()> {
    let mut embed = Embed2::warning()
        .title("nhentai")
        .description(message.to_string())
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

fn extract_id(ctx: &CommandContext) -> Option<u64> {
    // 1. Explicit args.
    let args = ctx.args().trim();
    if !args.is_empty() {
        if let Some(id) = parse_gallery_id(args) {
            return Some(id);
        }
    }
    // 2. Reply target (prefix invocations only — slash has no implicit reply).
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if let Some(referenced) = msg.referenced_message.as_deref() {
            if let Some(id) = parse_gallery_id(&referenced.content) {
                return Some(id);
            }
        }
    }
    // 3. Walk back through cached non-bot messages.
    cache_pick_from_human(&ctx.bot, ctx.channel_id(), parse_gallery_id)
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}
