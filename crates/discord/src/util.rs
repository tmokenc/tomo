//! Small utilities shared by command implementations. The truncation helper
//! is re-exported from [`tomo_embed`] so call-sites only depend on one path.

pub use tomo_embed::truncate;

use bytes::Bytes;
use tokio::task;
use tracing::{debug, warn};
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::{Attachment, Message};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, MessageMarker, UserMarker};

use tomo_core::error::{Error, Result};

use crate::command::{CommandContext, InvocationSource};
use crate::ocr_merge;
use crate::state::{Bot, OcrSlot};

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
        // Per message we prefer attachments over embeds (uploads are more
        // intentional than auto-embeds from posted links). Across messages
        // we still walk newest-first, so the most recent image wins.
        if let Some(url) = first_image_attachment(&msg.attachments) {
            return Some(url);
        }
        if let Some(url) = first_image_embed(&msg) {
            return Some(url);
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

/// Walk back up to [`IMAGE_LOOKBACK`] recent cached messages in `channel`
/// and run `pick` against the *content* of every non-bot human message,
/// stopping at the first hit. Returns `None` if nothing matches or the
/// cache doesn't have history for the channel.
///
/// Used by the `nhentai` / `ehentai` commands to find a referenced gallery
/// when the user invoked the command without arguments and without a reply.
pub fn cache_pick_from_human<F, T>(bot: &Bot, channel: Id<ChannelMarker>, mut pick: F) -> Option<T>
where
    F: FnMut(&str) -> Option<T>,
{
    let messages = bot.cache.channel_messages(channel)?;
    let message_ids: Vec<_> = messages.iter().rev().take(IMAGE_LOOKBACK).copied().collect();
    drop(messages);
    for message_id in message_ids {
        let Some(msg) = bot.cache.message(message_id) else { continue };
        if is_bot_user(bot, msg.author.id) {
            continue;
        }
        if let Some(value) = pick(&msg.content) {
            return Some(value);
        }
    }
    None
}

/// Cache-backed bot check. Returns false on cache miss so we err on the
/// side of *trying* to parse content rather than silently dropping it.
pub fn is_bot_user(bot: &Bot, user_id: Id<UserMarker>) -> bool {
    bot.cache.user(user_id).map(|u| u.bot).unwrap_or(false)
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

/// Variant of [`find_image_url`] that operates on a [`Message`] directly,
/// for callers that don't have a [`CommandContext`] (e.g. the LLM-mention
/// handler). Walks: attachments on the message → reply target → message
/// embeds → recent cached channel messages.
pub fn find_image_url_in_message(bot: &Bot, message: &Message) -> Option<String> {
    if let Some(url) = first_image_attachment(&message.attachments) {
        return Some(url);
    }
    if let Some(referenced) = message.referenced_message.as_deref() {
        if let Some(url) = first_image_attachment(&referenced.attachments) {
            return Some(url);
        }
        if let Some(url) = first_image_embed(referenced) {
            return Some(url);
        }
    }
    if let Some(url) = first_image_embed(message) {
        return Some(url);
    }
    cache_lookback(bot, message.channel_id)
}

/// Run every configured OCR engine over `bytes`, merge their bounding
/// boxes spatially via [`crate::ocr_merge`], and return the joined text
/// as a single string. Empty if no engines are set up or the engines
/// produced nothing. Errors bubble up only when image decoding fails —
/// individual engine misses are logged and swallowed by `run_merged`.
pub async fn ocr_bytes(engines: Vec<OcrSlot>, bytes: Bytes) -> Result<String> {
    if engines.is_empty() {
        return Ok(String::new());
    }
    let bytes = bytes.to_vec();
    let lines = task::spawn_blocking(move || ocr_merge::run_merged(&engines, &bytes))
        .await
        .map_err(|e| Error::config(format!("ocr join: {e}")))??;
    Ok(ocr_merge::render(&lines))
}

/// Strip Discord's auto-generated link previews from a user's message.
/// The gallery commands and their auto-triggers call this after posting
/// their own rich embed, so a `https://vndb.org/v123` paste doesn't end
/// up rendered twice (once by Discord, once by us).
///
/// Requires Manage Messages on the channel — the bot is editing someone
/// *else's* message. Failures are logged at debug and otherwise ignored
/// because losing the suppression doesn't break anything user-visible.
pub async fn suppress_embeds(
    bot: &Bot,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
) {
    if let Err(e) = bot
        .http
        .update_message(channel_id, message_id)
        .flags(MessageFlags::SUPPRESS_EMBEDS)
        .await
    {
        debug!(
            channel = %channel_id,
            message = %message_id,
            error = %e,
            "suppress_embeds failed (missing Manage Messages?)",
        );
    }
}

/// Whether *every* URL in `content` points at one of `domains` (and there is
/// at least one URL). Gates [`suppress_embeds`]: Discord's `SUPPRESS_EMBEDS`
/// flag hides **all** of a message's embeds, not just one, so we must only
/// suppress when the only links present are the ones we just mirrored.
/// Otherwise pasting e.g. `look at v5 https://youtube.com/...` would nuke the
/// unrelated YouTube preview. Returns `false` when there are no URLs (nothing
/// to suppress) or when any URL is off-domain.
pub fn message_only_links_to(content: &str, domains: &[&str]) -> bool {
    let mut saw_url = false;
    // Scan *every* `scheme://` in the whole string, not one per whitespace
    // token — two URLs joined without a space (`a://x,b://y`) must both be
    // checked, or an off-domain preview could slip past the gate.
    let mut rest = content;
    while let Some(scheme_pos) = rest.find("://") {
        saw_url = true;
        let after = &rest[scheme_pos + 3..];
        // Host is everything up to the first separator. Note we do NOT split
        // on `@`, so a deceptive `https://vndb.org@evil.com/` yields the host
        // `vndb.org@evil.com`, which won't match — erring toward *not*
        // suppressing, which is the safe direction.
        let host = after
            .split([
                '/', '?', '#', ':', ',', ';', ' ', '\t', '\n', '\r', ')', ']', '}', '"', '\'',
                '<', '>', '|',
            ])
            .next()
            .unwrap_or("");
        let host = host.strip_prefix("www.").unwrap_or(host).to_ascii_lowercase();
        let on_domain = domains
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")));
        if !on_domain {
            return false;
        }
        rest = after;
    }
    saw_url
}

#[cfg(test)]
mod tests {
    use super::message_only_links_to;

    #[test]
    fn no_url_means_no_suppression() {
        assert!(!message_only_links_to("just v5 here", &["vndb.org"]));
        assert!(!message_only_links_to("177013", &["nhentai.net"]));
    }

    #[test]
    fn matching_url_suppresses() {
        assert!(message_only_links_to("https://vndb.org/v5", &["vndb.org"]));
        assert!(message_only_links_to(
            "see https://www.nhentai.net/g/177013/",
            &["nhentai.net"]
        ));
    }

    #[test]
    fn unrelated_url_blocks_suppression() {
        // The reported bug: a YouTube link next to our resource must NOT be
        // suppressed (SUPPRESS_EMBEDS would nuke all previews).
        assert!(!message_only_links_to(
            "look at v5 https://youtube.com/watch?v=abc",
            &["vndb.org"]
        ));
    }

    #[test]
    fn all_urls_must_match() {
        assert!(message_only_links_to(
            "https://vndb.org/v5 and https://vndb.org/v17",
            &["vndb.org"]
        ));
        assert!(!message_only_links_to(
            "https://vndb.org/v5 and https://example.com",
            &["vndb.org"]
        ));
    }

    #[test]
    fn subdomain_matches_but_lookalike_does_not() {
        assert!(message_only_links_to("https://img.vndb.org/x", &["vndb.org"]));
        // `notvndb.org` must not be treated as a vndb.org subdomain.
        assert!(!message_only_links_to("https://notvndb.org/x", &["vndb.org"]));
    }

    #[test]
    fn examines_every_url_even_without_whitespace() {
        // Two URLs glued together by a comma form one whitespace-token; the
        // off-domain one must still block suppression.
        assert!(!message_only_links_to(
            "https://vndb.org/v5,https://youtube.com/x",
            &["vndb.org"]
        ));
    }

    #[test]
    fn userinfo_lookalike_does_not_match() {
        // `vndb.org` in the userinfo position must not be treated as the host.
        assert!(!message_only_links_to("https://vndb.org@evil.com/x", &["vndb.org"]));
    }
}
