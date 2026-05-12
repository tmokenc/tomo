//! When a non-bot user `@`s the bot, fetch a short conversation history and
//! ask Gemini for a reply.

use std::sync::atomic::Ordering;

use chrono::Datelike;
use tracing::{debug, info, warn};
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

use tomo_core::error::Result;
use tomo_llm::{GenerateError, Role, Turn};

use crate::state::Bot;

/// True when the bot should respond.
///
/// We consider any of these a mention:
///   * a real `<@bot_id>` / `<@!bot_id>` user-mention in `message.mentions`;
///   * the same token literally present in `message.content` (covers the
///     case where `mentions` is empty because the `MESSAGE_CONTENT` intent
///     resolved the mention without the parsed sidecar, or twilight failed
///     to populate it);
///   * a `<@&role_id>` role mention referencing one of the bot member's
///     roles (e.g. someone @-roles "Bots");
///   * a reply to a message authored by the bot (Discord's "ping replies"
///     feature — keeps conversations flowing without re-@).
///
/// Logs the decision at `info` level so it's possible to diagnose
/// non-responses from production logs without flipping `RUST_LOG=debug`.
pub fn should_respond(message: &Message, bot: &Bot) -> bool {
    if message.author.bot {
        return false;
    }
    let bot_id = bot.identity.user_id;
    let bot_uid = bot_id.get();

    // 1. Direct user mention via the parsed `mentions` array.
    if message.mentions.iter().any(|m| m.id == bot_id) {
        info!(
            user = %message.author.id,
            channel = %message.channel_id,
            "llm mention: matched via mentions[]"
        );
        return true;
    }

    // 2. Raw-content fallback. Discord embeds the mention as `<@id>` and
    // (legacy) `<@!id>` in the message text. If `mentions` is empty but the
    // text contains either form, treat it as a mention. This catches the
    // case where the `mentions` sidecar is dropped or stripped — the
    // gateway has been seen to do this when the bot lacks
    // MESSAGE_CONTENT in some shapes.
    if mentions_in_content(&message.content, bot_uid) {
        info!(
            user = %message.author.id,
            channel = %message.channel_id,
            "llm mention: matched via content scan"
        );
        return true;
    }

    // 3. Role mention of any role the bot has in this guild.
    if let Some(guild_id) = message.guild_id {
        if !message.mention_roles.is_empty() {
            if let Some(member) = bot.cache.member(guild_id, bot_id) {
                if message.mention_roles.iter().any(|r| member.roles.contains(r)) {
                    info!(
                        user = %message.author.id,
                        channel = %message.channel_id,
                        "llm mention: matched via role mention"
                    );
                    return true;
                }
            }
        }
    }

    // 4. Reply to a message authored by the bot.
    if let Some(ref_msg) = message.referenced_message.as_deref() {
        if ref_msg.author.id == bot_id {
            info!(
                user = %message.author.id,
                channel = %message.channel_id,
                "llm mention: matched via reply-to-bot"
            );
            return true;
        }
    }

    debug!(
        user = %message.author.id,
        channel = %message.channel_id,
        content_len = message.content.len(),
        mentions = message.mentions.len(),
        role_mentions = message.mention_roles.len(),
        reply = message.referenced_message.is_some(),
        "llm mention: no match (not responding)"
    );
    false
}

/// `<@id>` or `<@!id>` somewhere in the content. We don't full-parse the
/// content because Discord can put extra characters between `<@` and the
/// digits in rare cases (badges in nicknames, etc.) — instead we look for
/// the digit sequence and check that it's bracketed by `<@` / `>`.
fn mentions_in_content(content: &str, bot_uid: u64) -> bool {
    if content.is_empty() {
        return false;
    }
    let target = bot_uid.to_string();
    let mut search = content;
    while let Some(idx) = search.find(&target) {
        // Need `<@` (optionally `<@!`) immediately before the id and `>`
        // immediately after.
        let before = &search[..idx];
        let after_id_start = idx + target.len();
        let after = &search[after_id_start..];
        let opens_correctly = before.ends_with("<@") || before.ends_with("<@!");
        let closes_correctly = after.starts_with('>');
        if opens_correctly && closes_correctly {
            return true;
        }
        search = &search[idx + target.len()..];
    }
    false
}

pub async fn handle(bot: Bot, message: Message) -> Result<()> {
    let Some(llm) = bot.llm.clone() else {
        debug!("llm mention ignored: router not configured");
        return Ok(());
    };

    if !llm.rate_limit.check(message.author.id) {
        debug!(user = %message.author.id, "llm rate-limit hit");
        return Ok(());
    }

    let user_text = strip_self_mention(&message.content, bot.identity.user_id.get());
    if user_text.trim().is_empty() {
        debug!("llm mention with no content after stripping the bot mention");
        return Ok(());
    }

    // Build the request history WITHOUT mutating the store yet. We only
    // commit a User+Model pair on a successful response, otherwise a failed
    // mention leaves a hanging user turn that breaks the next request.
    let prior = llm.conversations.snapshot(message.channel_id);
    let mut request_turns =
        trim_for_request(prior, llm.router.context_messages.saturating_sub(1));
    request_turns.push(Turn { role: Role::User, text: user_text.clone() });

    let mut extra_context = build_context(&bot, &message);
    if let Some(ocr_block) = build_ocr_context(&bot, &message).await {
        extra_context.push('\n');
        extra_context.push_str(&ocr_block);
    }
    debug!(
        chars = extra_context.len(),
        "llm: appending per-request context to system prompt"
    );

    // Typing indicator while we wait on the model.
    let _ = bot.http.create_typing_trigger(message.channel_id).await;

    let response = match llm.router.generate(&request_turns, Some(&extra_context)).await {
        Ok(r) => r,
        Err(GenerateError::QuotaExceeded { .. }) => {
            // Every provider/model in the chain is currently cooled-down or
            // just 429'd. Silent to the user; loud to operators once a day.
            debug!("llm quota exhausted across all providers; staying silent");
            maybe_alert_quota(&bot).await;
            return Ok(());
        }
        Err(e) => {
            // Non-quota errors used to be silent — that left users staring at
            // a no-op @-mention. We still don't echo internals, but we log
            // loudly and react with ⚠ so it's visible something failed.
            warn!(error = %e, user = %message.author.id, "llm generate failed");
            react_warn(&bot, &message).await;
            return Ok(());
        }
    };

    if response.text.trim().is_empty() {
        // Empty content (safety filter, refusal, …). Same treatment as a
        // hard error: visible signal, no echo.
        warn!(
            user = %message.author.id,
            provider = %response.provider_used,
            model = %response.model_used,
            "llm returned no text (likely safety filtered)"
        );
        react_warn(&bot, &message).await;
        return Ok(());
    }

    info!(
        provider = %response.provider_used,
        model = %response.model_used,
        prompt_tokens = response.prompt_tokens,
        output_tokens = response.output_tokens,
        "llm: served mention"
    );

    // Persist both turns now that we have a real reply.
    llm.conversations
        .push(message.channel_id, Turn { role: Role::User, text: user_text });
    llm.conversations.push(
        message.channel_id,
        Turn { role: Role::Model, text: response.text.clone() },
    );

    bot.stats.record_gemini(
        message.author.id,
        message.guild_id,
        response.prompt_tokens,
        response.output_tokens,
    );

    let chunks = chunk_for_discord(&response.text, 1900);
    for (i, chunk) in chunks.into_iter().enumerate() {
        let mut req = bot.http.create_message(message.channel_id).content(&chunk);
        if i == 0 {
            req = req.reply(message.id);
        }
        if let Err(e) = req.await {
            warn!(error = %e, "send llm reply");
            break;
        }
    }

    Ok(())
}

/// Drop oldest turns until the history both fits the budget and starts on a
/// user turn. Gemini rejects requests that begin with a model turn ("role
/// must be 'user'"), so this is load-bearing for any conversation past the
/// `context_messages` cap.
fn trim_for_request(mut turns: Vec<Turn>, max_prior: usize) -> Vec<Turn> {
    if turns.len() > max_prior {
        let drop = turns.len() - max_prior;
        turns.drain(..drop);
    }
    while let Some(first) = turns.first() {
        if first.role == Role::User {
            break;
        }
        turns.remove(0);
    }
    turns
}

/// React with ⚠ on the user's message so they know we tried but something
/// failed. Suppressed if reactions can't be added (missing perms etc.) —
/// never surfaced to the user.
async fn react_warn(bot: &Bot, message: &Message) {
    let request =
        twilight_http::request::channel::reaction::RequestReactionType::Unicode { name: "⚠️" };
    if let Err(e) = bot
        .http
        .create_reaction(message.channel_id, message.id, &request)
        .await
    {
        debug!(error = %e, "could not add warn reaction");
    }
}

/// DM each bot owner if today is the first quota hit we've seen. Atomic CAS
/// on `BotState::llm_quota_alert_day` ensures only one DM goes out per
/// UTC day even under concurrent quota errors.
async fn maybe_alert_quota(bot: &Bot) {
    let today = chrono::Utc::now().date_naive().num_days_from_ce() as i64;
    let prev = bot.llm_quota_alert_day.load(Ordering::Acquire);
    if prev == today {
        return;
    }
    if bot
        .llm_quota_alert_day
        .compare_exchange(prev, today, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return; // another task got there first
    }

    info!("DMing owners — gemini quota exhausted for today");
    let body = "⚠️ Gemini quota / rate limit hit. Mentions are being silently dropped \
                until the quota resets. (You'll only see this once per day.)";
    for owner_id in bot.owners.iter().copied() {
        if let Err(e) = dm(bot, owner_id, body).await {
            warn!(owner = %owner_id, error = %e, "could not DM owner about quota");
        }
    }
}

async fn dm(bot: &Bot, user_id: Id<UserMarker>, body: &str) -> Result<()> {
    use tomo_core::error::Error;
    let channel = bot
        .http
        .create_private_channel(user_id)
        .await
        .map_err(|e| Error::config(format!("create dm channel: {e}")))?
        .model()
        .await
        .map_err(|e| Error::config(format!("decode dm channel: {e}")))?;
    bot.http
        .create_message(channel.id)
        .content(body)
        .await
        .map_err(|e| Error::config(format!("send dm: {e}")))?;
    Ok(())
}

fn strip_self_mention(content: &str, bot_user_id: u64) -> String {
    let bare = format!("<@{bot_user_id}>");
    let nick = format!("<@!{bot_user_id}>");
    content
        .replace(&bare, " ")
        .replace(&nick, " ")
        .trim()
        .to_string()
}

/// Build the per-request context block appended to the configured system
/// prompt. Lists where the message came from (server / channel / topic /
/// DMs), who sent it, plus the current UTC date — all things the static
/// prompt can't know.
///
/// Kept compact (one line per fact) to minimise input-token cost.
fn build_context(bot: &Bot, message: &Message) -> String {
    let mut lines = Vec::with_capacity(6);

    // Where: guild + channel.
    match message.guild_id {
        Some(guild_id) => {
            let guild_name = bot
                .cache
                .guild(guild_id)
                .map(|g| g.name.clone())
                .unwrap_or_else(|| format!("guild {guild_id}"));
            lines.push(format!("Server: {guild_name}"));

            if let Some(channel) = bot.cache.channel(message.channel_id) {
                if let Some(name) = channel.name.as_deref().filter(|s| !s.is_empty()) {
                    lines.push(format!("Channel: #{name}"));
                }
                if let Some(topic) = channel.topic.as_deref().filter(|s| !s.is_empty()) {
                    lines.push(format!("Channel topic: {}", truncate(topic, 400)));
                }
                if channel.nsfw.unwrap_or(false) {
                    lines.push("Channel is marked NSFW.".to_string());
                }
            }
        }
        None => {
            lines.push("Conversation is in a direct message (no server).".to_string());
        }
    }

    // Who: prefer the guild member nick when there is one, fall back to the
    // global display name, then username.
    let speaker = speaker_label(bot, message);
    lines.push(format!("User mentioning you: {speaker}"));

    // If this is a Discord reply, surface the message being replied to so the
    // model can answer in context. Without this, "@tomo translate that"
    // attached as a reply to some earlier message reads as a non-sequitur to
    // the model.
    if let Some(referenced) = message.referenced_message.as_deref() {
        let bot_id = bot.identity.user_id;
        let author = if referenced.author.id == bot_id {
            "you (your previous message)".to_string()
        } else {
            speaker_for(bot, referenced)
        };
        let body = referenced.content.replace('\n', " ");
        lines.push(format!(
            "Replying to message from {author}: \"{}\"",
            truncate(&body, 500),
        ));
    }

    // When: gives the model a current-time anchor independent of training cut-off.
    lines.push(format!(
        "Current time: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M UTC")
    ));

    lines.join("\n")
}

/// Best-effort OCR pass over any image attached to the invoking message, its
/// reply target, or recent cached messages in the channel — whichever turns
/// up first. The recognised text is wrapped in a labelled block and returned
/// for inclusion in the system-prompt context.
///
/// Returns `None` when:
/// * no OCR engines are configured, or
/// * no image is reachable, or
/// * fetch/OCR fails (logged at `debug!` — never bubbles up to the user), or
/// * OCR produced no text, or
/// * the OCR work exceeded the budget (caller stays snappy).
async fn build_ocr_context(bot: &Bot, message: &Message) -> Option<String> {
    if bot.ocr.is_empty() {
        return None;
    }
    let url = crate::util::find_image_url_in_message(bot, message)?;
    let bytes = match crate::util::fetch_image_bytes(bot, &url).await {
        Ok(b) => b,
        Err(e) => {
            debug!(url, error = %e, "llm: image fetch for OCR failed");
            return None;
        }
    };

    let engines = bot.ocr.clone();
    // Cap OCR latency so a slow image never makes the bot look frozen on
    // mention. 8 s easily covers PaddleOCR on a CPU for typical screenshots.
    let ocr_fut = crate::util::ocr_bytes(engines, bytes);
    let text = match tokio::time::timeout(std::time::Duration::from_secs(8), ocr_fut).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            debug!(url, error = %e, "llm: OCR failed");
            return None;
        }
        Err(_) => {
            debug!(url, "llm: OCR timed out — skipping");
            return None;
        }
    };

    let cleaned = text.trim();
    if cleaned.is_empty() {
        return None;
    }
    // Hard cap on tokens-worth of OCR'd text.
    let body = if cleaned.chars().count() > 1500 {
        let head: String = cleaned.chars().take(1500).collect();
        format!("{head}…")
    } else {
        cleaned.to_string()
    };
    Some(format!(
        "Image attached to the conversation. OCR-extracted text follows (may contain noise):\n```\n{body}\n```"
    ))
}

/// Same logic as [`speaker_label`] but generic over a [`Message`] (so it can
/// describe the replied-to message's author, not just `message.author`).
fn speaker_for(bot: &Bot, message: &Message) -> String {
    let name = if let Some(guild_id) = message.guild_id {
        bot.cache
            .member(guild_id, message.author.id)
            .and_then(|m| m.nick.clone())
            .or_else(|| message.author.global_name.clone())
            .unwrap_or_else(|| message.author.name.clone())
    } else {
        message
            .author
            .global_name
            .clone()
            .unwrap_or_else(|| message.author.name.clone())
    };
    if name.chars().count() > 80 {
        let head: String = name.chars().take(80).collect();
        format!("{head}…")
    } else {
        name
    }
}

/// Best-effort human label for the mention author. Prefers a guild nickname,
/// falls back to the global display name, finally the bare username.
fn speaker_label(bot: &Bot, message: &Message) -> String {
    let name = if let Some(guild_id) = message.guild_id {
        bot.cache
            .member(guild_id, message.author.id)
            .and_then(|m| m.nick.clone())
            .or_else(|| message.author.global_name.clone())
            .unwrap_or_else(|| message.author.name.clone())
    } else {
        message
            .author
            .global_name
            .clone()
            .unwrap_or_else(|| message.author.name.clone())
    };
    // Truncate aggressively — usernames in the wild can be huge after
    // Unicode shenanigans, and we're paying tokens for this.
    if name.chars().count() > 80 {
        let head: String = name.chars().take(80).collect();
        format!("{head}…")
    } else {
        name
    }
}

/// Local re-export so we don't pull `util::truncate` into a runtime path.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn chunk_for_discord(text: &str, max_len: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if current.len() + line.len() > max_len && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if line.len() > max_len {
            // A single absurdly long line — hard split.
            for chunk in split_long(line, max_len) {
                out.push(chunk);
            }
        } else {
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn split_long(s: &str, max_len: usize) -> Vec<String> {
    s.as_bytes()
        .chunks(max_len)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::mentions_in_content;

    #[test]
    fn detects_plain_user_mention() {
        assert!(mentions_in_content("<@12345> hi", 12345));
    }

    #[test]
    fn detects_legacy_nickname_mention() {
        assert!(mentions_in_content("<@!12345> hi", 12345));
    }

    #[test]
    fn ignores_partial_id_match() {
        // 12345 is a prefix of 123456 — must not match.
        assert!(!mentions_in_content("<@123456> hi", 12345));
    }

    #[test]
    fn ignores_bare_id_without_brackets() {
        assert!(!mentions_in_content("user 12345 said", 12345));
    }

    #[test]
    fn detects_mention_anywhere_in_content() {
        assert!(mentions_in_content("hey there <@99> what's up", 99));
    }
}
