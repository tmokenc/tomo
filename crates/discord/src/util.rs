//! Small utilities shared by command implementations. The truncation helper
//! is re-exported from [`tomo_embed`] so call-sites only depend on one path.

pub use tomo_embed::truncate;

use bytes::Bytes;
use tracing::warn;
use twilight_model::channel::{Attachment, Message};
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;

use tomo_core::error::{Error, Result};

use crate::command::{CommandContext, InvocationSource};
use crate::state::Bot;

/// How many recent cached messages to walk when looking for an image
/// attachment in the channel history.
pub const IMAGE_LOOKBACK: usize = 20;

/// Find a URL pointing at an image the command should operate on. Priorities,
/// in order:
/// 1. attachments on the invoking message
/// 2. the message it replied to (attachments, then embed image)
/// 3. embeds on the invoking message
/// 4. up to [`IMAGE_LOOKBACK`] recent cached messages in the channel
///
/// Returns `None` if nothing usable is found. Slash commands fall straight
/// through to the cache lookback since they have no attached message of
/// their own through the prefix-style invocation flow.
pub fn find_image_url(ctx: &CommandContext) -> Option<String> {
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if let Some(url) = first_image_attachment(&msg.attachments) {
            return Some(url);
        }
        if let Some(referenced) = msg.referenced_message.as_deref() {
            if let Some(url) = first_image_attachment(&referenced.attachments) {
                return Some(url);
            }
            if let Some(url) = first_image_embed(referenced) {
                return Some(url);
            }
        }
        if let Some(url) = first_image_embed(msg) {
            return Some(url);
        }
    }
    cache_lookback(&ctx.bot, ctx.channel_id())
}

fn first_image_attachment(attachments: &[Attachment]) -> Option<String> {
    attachments
        .iter()
        .find(|a| {
            a.content_type
                .as_deref()
                .map(|t| t.starts_with("image/"))
                .unwrap_or(false)
        })
        .map(|a| a.url.clone())
}

fn first_image_embed(msg: &Message) -> Option<String> {
    for embed in &msg.embeds {
        if let Some(image) = &embed.image {
            return Some(image.url.clone());
        }
        if let Some(thumb) = &embed.thumbnail {
            return Some(thumb.url.clone());
        }
    }
    None
}

fn cache_lookback(bot: &Bot, channel_id: Id<ChannelMarker>) -> Option<String> {
    let messages = bot.cache.channel_messages(channel_id)?;
    let ids: Vec<_> = messages.iter().rev().take(IMAGE_LOOKBACK).copied().collect();
    drop(messages);
    for message_id in ids {
        let Some(msg) = bot.cache.message(message_id) else { continue };
        for attachment in msg.attachments() {
            if attachment
                .content_type
                .as_deref()
                .map(|t| t.starts_with("image/"))
                .unwrap_or(false)
            {
                return Some(attachment.url.clone());
            }
        }
    }
    None
}

/// Download an image from a URL via the bot's shared reqwest client. Returns
/// raw bytes so callers can hand off to `image::load_from_memory` etc.
pub async fn fetch_image_bytes(bot: &Bot, url: &str) -> Result<Bytes> {
    let resp = bot
        .requester
        .http()
        .get(url)
        .send()
        .await
        .map_err(|e| Error::config(format!("fetch image: {e}")))?;
    let resp = resp
        .error_for_status()
        .map_err(|e| Error::config(format!("fetch image status: {e}")))?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::config(format!("read image bytes: {e}")))?;
    Ok(bytes)
}

/// Helper that wraps both lookup + download. Returns `Ok(None)` if no image
/// could be found (so callers can render a friendly "give me an image"
/// reply); returns `Err` only on real IO failures.
pub async fn fetch_image_from_context(ctx: &CommandContext) -> Result<Option<Bytes>> {
    let Some(url) = find_image_url(ctx) else {
        return Ok(None);
    };
    match fetch_image_bytes(&ctx.bot, &url).await {
        Ok(b) => Ok(Some(b)),
        Err(e) => {
            warn!(url, error = %e, "failed to fetch image");
            Err(e)
        }
    }
}
