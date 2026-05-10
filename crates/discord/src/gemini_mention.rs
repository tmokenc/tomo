//! When a non-bot user `@`s the bot, fetch a short conversation history and
//! ask Gemini for a reply.

use std::sync::atomic::Ordering;

use chrono::Datelike;
use tracing::{debug, info, warn};
use twilight_model::channel::Message;
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

use tomo_core::error::Result;
use tomo_gemini::{GenerateError, Role, Turn};

use crate::state::Bot;

/// True when the bot should respond — non-bot author who actually mentions us.
pub fn should_respond(message: &Message, bot: &Bot) -> bool {
    if message.author.bot {
        return false;
    }
    message.mentions.iter().any(|m| m.id == bot.identity.user_id)
}

pub async fn handle(bot: Bot, message: Message) -> Result<()> {
    let Some(gemini) = bot.gemini.clone() else { return Ok(()); };

    if !gemini.rate_limit.check(message.author.id) {
        debug!(user = %message.author.id, "gemini rate-limit hit");
        return Ok(());
    }

    let user_text = strip_self_mention(&message.content, bot.identity.user_id.get());
    if user_text.trim().is_empty() {
        // Nothing to ask.
        return Ok(());
    }

    gemini
        .conversations
        .push(message.channel_id, Turn { role: Role::User, text: user_text.clone() });

    let history = gemini.conversations.snapshot(message.channel_id);
    let history = trim_history(history, gemini.client.config().context_messages);

    // Typing indicator while we wait on the model.
    let _ = bot.http.create_typing_trigger(message.channel_id).await;

    let response = match gemini.client.generate(&history).await {
        Ok(r) => r,
        Err(GenerateError::QuotaExceeded) => {
            // Silent to the user — only the operators see this.
            debug!("gemini quota exhausted; staying silent");
            maybe_alert_quota(&bot).await;
            return Ok(());
        }
        Err(e) => {
            warn!(error = %e, "gemini generate failed");
            return Ok(());
        }
    };

    if response.text.trim().is_empty() {
        return Ok(());
    }

    gemini.conversations.push(message.channel_id, Turn {
        role: Role::Model,
        text: response.text.clone(),
    });

    bot.stats
        .record_gemini(message.author.id, message.guild_id, response.prompt_tokens, response.output_tokens);

    let chunks = chunk_for_discord(&response.text, 1900);
    for (i, chunk) in chunks.into_iter().enumerate() {
        let mut req = bot.http.create_message(message.channel_id).content(&chunk);
        if i == 0 {
            req = req.reply(message.id);
        }
        if let Err(e) = req.await {
            warn!(error = %e, "send gemini reply");
            break;
        }
    }

    Ok(())
}

/// DM each bot owner if today is the first quota hit we've seen. Atomic CAS
/// on `BotState::gemini_quota_alert_day` ensures only one DM goes out per
/// UTC day even under concurrent quota errors.
async fn maybe_alert_quota(bot: &Bot) {
    let today = chrono::Utc::now().date_naive().num_days_from_ce() as i64;
    let prev = bot.gemini_quota_alert_day.load(Ordering::Acquire);
    if prev == today {
        return;
    }
    if bot
        .gemini_quota_alert_day
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

fn trim_history(mut turns: Vec<Turn>, max: usize) -> Vec<Turn> {
    let cap = max.max(1);
    if turns.len() > cap {
        let drop = turns.len() - cap;
        turns.drain(..drop);
    }
    turns
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
