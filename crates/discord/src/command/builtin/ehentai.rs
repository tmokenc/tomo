//! `ehentai <urls or g/<gid>/<token>>` — look up one or more E-Hentai
//! galleries on the `gdata` JSON-RPC endpoint. NSFW-channel-only; up to 25
//! galleries per call (the upstream cap).

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::{Error, Result};
use tomo_embed::Embed2;
use tomo_requester::ehentai::parse_ids;

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
use crate::ehentai_view::gallery_embed;
use crate::util::{cache_pick_from_human, truncate};

/// Site favicon used to brand the embed footer when we render the
/// multi-gallery list.
const EH_ICON: &str = "https://e-hentai.org/favicon.ico";

pub struct EhentaiCommand;

#[async_trait]
impl Command for EhentaiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "ehentai",
                "Show gallery info for E-Hentai URLs or bare `g/<gid>/<token>/` paths.",
            )
            .aliases(["eh", "e-hentai", "sadkaede", "sadpanda"])
            .category("Search")
            .guild_only()
            .string_option(
                "url",
                "E-Hentai/ExHentai URL or bare `g/<gid>/<token>/` (multiple allowed, space-separated)",
                true,
            )
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        if !is_nsfw_channel(&ctx) {
            return reply_warn(&ctx, "That command is NSFW-only.").await;
        }

        let ids = gather_ids(&ctx);
        if ids.is_empty() {
            return reply_warn(
                &ctx,
                "Give me one or more `e-hentai.org/g/<gid>/<token>/` URLs or \
                 bare `g/<gid>/<token>/` paths.",
            )
            .await;
        }

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;
        let galleries = match ctx.bot.requester.ehentai_gmetadata(&ids).await {
            Ok(g) => g,
            Err(e) => {
                let mut err = Embed2::error()
                    .title("E-Hentai")
                    .description(format!("Lookup failed: `{e}`"))
                    .footer_with("e-hentai.org", EH_ICON)
                    .timestamp_now();
                if let Some(user) = ctx.author() {
                    err = err.author_user(user);
                }
                return ctx.reply_embed(&err).await;
            }
        };
        if galleries.is_empty() {
            return reply_warn(&ctx, "Nothing came back.").await;
        }

        // For >1 result, render a compact list. For one, render rich details.
        if galleries.len() == 1 {
            let embed = gallery_embed(&galleries[0], ctx.author()).build();
            ctx.bot
                .http
                .create_message(ctx.channel_id())
                .embeds(std::slice::from_ref(&embed))
                .await
                .map_err(|e| Error::config(format!("ehentai send: {e}")))?;
            Ok(())
        } else {
            let mut body = String::with_capacity(galleries.len() * 80);
            let mut rendered = 0usize;
            for g in &galleries {
                let title = g.title.as_deref().or(g.title_jpn.as_deref()).unwrap_or("(no title)");
                let entry = format!("• [{title}]({})\n", g.url());
                if body.len() + entry.len() > 3500 {
                    body.push_str("…\n");
                    break;
                }
                body.push_str(&entry);
                rendered += 1;
            }
            let mut embed = Embed2::lovely()
                .title(format!("{} galleries", galleries.len()))
                .description(truncate(&body, 4000))
                .field_inline("Shown", rendered.to_string())
                .field_inline("Total", galleries.len().to_string())
                .footer_with("e-hentai.org", EH_ICON)
                .timestamp_now();
            if let Some(user) = ctx.author() {
                embed = embed.author_user(user);
            }
            ctx.reply_embed(&embed).await
        }
    }
}

async fn reply_warn(ctx: &CommandContext, message: &str) -> Result<()> {
    let mut embed = Embed2::warning()
        .title("E-Hentai")
        .description(message.to_string())
        .footer_with("e-hentai.org", EH_ICON)
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

fn gather_ids(ctx: &CommandContext) -> Vec<(u64, String)> {
    // 1. Explicit args.
    let args = ctx.args();
    let mut ids = parse_ids(args);
    if !ids.is_empty() {
        ids.sort();
        ids.dedup();
        return ids;
    }
    // 2. Reply target.
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if let Some(referenced) = msg.referenced_message.as_deref() {
            let mut from_reply = parse_ids(&referenced.content);
            if !from_reply.is_empty() {
                from_reply.sort();
                from_reply.dedup();
                return from_reply;
            }
        }
    }
    // 3. Walk back through cached non-bot messages — first one containing
    //    a usable `g/<gid>/<token>/` pattern wins.
    cache_pick_from_human(&ctx.bot, ctx.channel_id(), |content| {
        let mut found = parse_ids(content);
        if found.is_empty() {
            None
        } else {
            found.sort();
            found.dedup();
            Some(found)
        }
    })
    .unwrap_or_default()
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}
