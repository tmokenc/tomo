//! Per-guild edit / delete logging.
//!
//! Tomo posts a colored embed to a guild-configured log channel whenever a
//! human user edits or deletes a message in that guild. Bot messages are
//! ignored. The destination is configured via `/settings`.
//!
//! The original content is fetched from `tomo-cache`'s message store. The
//! caller is responsible for snapshotting it **before** `cache.update(&event)`
//! runs — by the time the runtime dispatcher calls into here, the cached
//! entry for a deleted message is gone. See `runtime::dispatch_event`.

use std::sync::Arc;

use tracing::{debug, warn};
use twilight_model::channel::message::Embed;
use twilight_model::channel::Message;
use twilight_model::gateway::payload::incoming::{MessageDelete, MessageUpdate};

use tomo_embed::{Color, Embed2};

use crate::state::Bot;
use crate::util::truncate;

/// Hidden-character placeholder for empty content so Discord doesn't reject
/// the embed field. (Bare `""` is dropped by `Embed2::field`.)
const EMPTY_PLACEHOLDER: &str = "*(empty)*";

/// Called from the runtime *after* `cache.update`. `pre_update` is the
/// snapshot the dispatcher captured before the cache mutation.
pub async fn handle_update(bot: Bot, event: MessageUpdate, pre_update: Option<Message>) {
    let Some(guild_id) = event.guild_id else {
        return; // DM edits — nothing to log to.
    };
    if event.author.bot {
        return;
    }
    let settings = bot.settings.get(guild_id).await;
    let Some(log_channel) = settings.log_channel else {
        return;
    };
    // Don't log edits *in* the log channel itself — that would either loop
    // forever (bot edits its own logs would also be ignored thanks to the
    // bot check above, but the visual noise is still bad) or just confuse.
    if log_channel == event.channel_id {
        return;
    }

    let new_content = event.content.clone();
    let old_content = pre_update.as_ref().map(|m| m.content.clone()).unwrap_or_default();

    // Filter out non-content edits: Discord sends MessageUpdate for embed
    // resolution on link-posts, pin/unpin, etc. If the text is identical
    // there's nothing to show.
    if old_content == new_content {
        debug!(
            message_id = %event.id,
            "message_log: skipping update with unchanged content"
        );
        return;
    }

    let jump = format!(
        "https://discord.com/channels/{guild_id}/{channel}/{msg}",
        channel = event.channel_id,
        msg = event.id,
    );

    // Per spec: don't render the author as an `<@id>` mention (avoids
    // sending an accidental ping even though embed-mentions don't notify
    // by default). The embed's author header already shows the username
    // + avatar; we record the id as plain text in the footer for audit.
    let mut embed = Embed2::new()
        .color(Color::MessageUpdate)
        .title("Message edited")
        .url(&jump)
        .author_user(&event.author)
        .field_block("Before", truncate(&maybe_empty(&old_content), 1024))
        .field_block("After", truncate(&maybe_empty(&new_content), 1024))
        .field_inline("Channel", format!("<#{}>", event.channel_id))
        .field_inline("Jump", format!("[link]({jump})"))
        .footer(format!("Message ID: {} · Author ID: {}", event.id, event.author.id))
        .timestamp_now();

    if let Some(prev) = pre_update.as_ref() {
        let removed: Vec<_> = prev
            .attachments
            .iter()
            .filter(|a| !event.attachments.iter().any(|b| b.id == a.id))
            .collect();
        if !removed.is_empty() {
            let body = removed
                .iter()
                .map(|a| format!("• `{}` ({} B)", a.filename, a.size))
                .collect::<Vec<_>>()
                .join("\n");
            embed = embed.field_block(
                format!("Attachment{} removed", if removed.len() == 1 { "" } else { "s" }),
                truncate(&body, 1024),
            );
        }
    }

    post(&bot, log_channel, embed.build()).await;
}

/// Called from the runtime *after* `cache.update`. The `MessageDelete`
/// event itself carries no content — we rely on the snapshot grabbed
/// pre-update.
pub async fn handle_delete(bot: Bot, event: MessageDelete, snapshot: Option<Message>) {
    let Some(guild_id) = event.guild_id else { return };
    let settings = bot.settings.get(guild_id).await;
    let Some(log_channel) = settings.log_channel else { return };
    if log_channel == event.channel_id {
        return;
    }

    let jump = format!(
        "https://discord.com/channels/{guild_id}/{channel}/{msg}",
        channel = event.channel_id,
        msg = event.id,
    );

    let mut embed = Embed2::new()
        .color(Color::MessageDelete)
        .title("Message deleted")
        .url(&jump)
        .field_inline("Channel", format!("<#{}>", event.channel_id))
        .footer(format!("Message ID: {}", event.id))
        .timestamp_now();

    match snapshot.as_ref() {
        Some(msg) if !msg.author.bot => {
            // No `<@id>` mention — the embed author header carries the
            // username + avatar, and we stamp the author id into the
            // footer for audit without rendering as a clickable mention.
            embed = embed
                .author_user(&msg.author)
                .field_block("Content", truncate(&maybe_empty(&msg.content), 1024));
            if !msg.attachments.is_empty() {
                let body = msg
                    .attachments
                    .iter()
                    .map(|a| format!("• [`{}`]({}) ({} B)", a.filename, a.url, a.size))
                    .collect::<Vec<_>>()
                    .join("\n");
                embed = embed.field_block(
                    format!("Attachment{}", if msg.attachments.len() == 1 { "" } else { "s" }),
                    truncate(&body, 1024),
                );
            }
            embed = embed.footer(format!(
                "Message ID: {} · Author ID: {}",
                event.id, msg.author.id
            ));
        }
        Some(_) => {
            // Bot's own message vanished — nothing to log per spec.
            return;
        }
        None => {
            // Cache miss — message was older than the cache window or
            // pre-bot-restart. Still useful to record the bare fact.
            embed = embed.description(
                "Original content not in cache. Only the message id + channel are known.",
            );
        }
    }

    post(&bot, log_channel, embed.build()).await;
}

fn maybe_empty(s: &str) -> String {
    if s.trim().is_empty() {
        EMPTY_PLACEHOLDER.to_string()
    } else {
        s.to_string()
    }
}

async fn post(bot: &Bot, channel: twilight_model::id::Id<twilight_model::id::marker::ChannelMarker>, embed: Embed) {
    let bot = Arc::clone(bot);
    let req = bot
        .http
        .create_message(channel)
        .embeds(std::slice::from_ref(&embed));
    if let Err(e) = req.await {
        // Most common failure: bot was kicked from the channel or the
        // channel was deleted but the setting wasn't cleared. Log loudly
        // once; subsequent failures will repeat but it's not worth
        // throttling here.
        warn!(channel = %channel, error = %e, "message_log: failed to post log embed");
    }
}
