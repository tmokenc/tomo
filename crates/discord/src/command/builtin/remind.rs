//! `remind <duration> <text>` / `remind list` / `remind remove <n>` —
//! mirrors `tomoka-rs`'s reminder command. State lives in `tomo-db` under
//! the `reminders` partition; a background task in `DiscordService::run`
//! delivers DMs when each reminder is due.

use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use humantime::parse_duration;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::reminder::{self, Reminder, MAX_DURATION, MAX_PER_USER};
use crate::util::truncate;

pub struct RemindCommand;

#[async_trait]
impl Command for RemindCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "remind",
                "Set a reminder. Use `list` to see them, `remove <n>` to cancel.",
            )
            .aliases(["remindme", "reminder"])
            .category("Utility")
            .string_option(
                "command",
                "Either `<duration> <message>` (e.g. `1h take out trash`), `list`, or `remove <n>`",
                true,
            )
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let args = ctx.args().trim();
        let first = args.split_whitespace().next().unwrap_or("");
        match first.to_ascii_lowercase().as_str() {
            "list" | "ls" => list_handler(&ctx).await,
            "remove" | "rm" | "del" | "delete" => {
                let rest = args[first.len()..].trim();
                remove_handler(&ctx, rest).await
            }
            _ => set_handler(&ctx, args).await,
        }
    }
}

async fn set_handler(ctx: &CommandContext, raw: &str) -> Result<()> {
    if raw.is_empty() {
        return reply_warn(
            ctx,
            "Reminder",
            "Usage: `remind <duration> <message>` — e.g. `remind 1h 30m take out trash`.",
        )
        .await;
    }

    let (duration, rest) = match consume_duration_prefix(raw) {
        Some(v) => v,
        None => {
            return reply_warn(ctx, "Reminder", &format!("I couldn't parse a duration from `{raw}`."))
                .await
        }
    };

    if duration > MAX_DURATION {
        return reply_warn(
            ctx,
            "Reminder",
            &format!(
                "That's more than the cap of `{}`.",
                humantime::format_duration(MAX_DURATION)
            ),
        )
        .await;
    }
    if duration.is_zero() {
        return reply_warn(ctx, "Reminder", "Duration must be greater than zero.").await;
    }
    let content = rest.trim();
    if content.is_empty() {
        return reply_warn(ctx, "Reminder", "Give me a message to remind you about.").await;
    }
    if content.len() > 1024 {
        return reply_warn(ctx, "Reminder", "Reminder text is too long (>1024 chars).").await;
    }

    let user_id = ctx.author_id().get();
    let existing = reminder::list_user(&ctx.bot, user_id).await?;
    if existing.len() >= MAX_PER_USER {
        return reply_warn(
            ctx,
            "Reminder",
            &format!("You already have {MAX_PER_USER} pending reminders — clear one first."),
        )
        .await;
    }

    let when_unix = Utc::now().timestamp() + duration.as_secs() as i64;
    let r = Reminder {
        id: reminder::fresh_id(),
        user_id,
        channel_id: ctx.channel_id().get(),
        guild_id: ctx.guild_id().map(|g| g.get()),
        when_unix,
        content: content.to_string(),
    };

    reminder::schedule(&ctx.bot, &r).await?;

    let pretty = humantime::format_duration(duration);
    let mut embed = Embed2::success()
        .title("Reminder set")
        .description(format!(
            "In **{pretty}** — I'll DM you.\nDelivery: <t:{when_unix}:F> (<t:{when_unix}:R>)",
        ))
        .field_block("Message", content.to_string())
        .field_inline("ID", format!("`{}`", r.id))
        .field_inline("Slots left", (MAX_PER_USER.saturating_sub(existing.len() + 1)).to_string())
        .field_inline("In", pretty.to_string())
        .footer(format!(
            "Cancel with `remind remove <n>` · max {MAX_PER_USER} per user"
        ))
        .timestamp_unix(when_unix);
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn list_handler(ctx: &CommandContext) -> Result<()> {
    let user_id = ctx.author_id().get();
    let entries = reminder::list_user(&ctx.bot, user_id).await?;
    if entries.is_empty() {
        return reply_warn(ctx, "Reminders", "You have no pending reminders.").await;
    }

    let mut body = String::with_capacity(entries.len() * 80);
    for (i, r) in entries.iter().enumerate() {
        body.push_str(&format!(
            "**{}.** <t:{}:R> · <t:{}:f>\n— {}\n",
            i + 1,
            r.when_unix,
            r.when_unix,
            truncate(&r.content, 160),
        ));
    }
    let next_when = entries.iter().map(|r| r.when_unix).min().unwrap_or(0);
    let mut embed = Embed2::info()
        .title("Your reminders")
        .description(body)
        .field_inline("Pending", entries.len().to_string())
        .field_inline("Slots left", (MAX_PER_USER.saturating_sub(entries.len())).to_string())
        .field_inline("Next", format!("<t:{next_when}:R>"))
        .footer(format!("Use `remind remove <n>` to cancel · max {MAX_PER_USER}"))
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn remove_handler(ctx: &CommandContext, rest: &str) -> Result<()> {
    let idx: usize = match rest.trim().parse() {
        Ok(n) if n >= 1 => n,
        _ => {
            return reply_warn(
                ctx,
                "Reminder",
                "Usage: `remind remove <n>` — the index from `remind list`.",
            )
            .await
        }
    };
    let user_id = ctx.author_id().get();
    let entries = reminder::list_user(&ctx.bot, user_id).await?;
    let Some(target) = entries.get(idx - 1) else {
        return reply_warn(ctx, "Reminder", "No reminder at that index.").await;
    };
    let removed_content = target.content.clone();
    let removed_when = target.when_unix;
    reminder::delete(&ctx.bot, target).await?;

    let mut embed = Embed2::success()
        .title("Reminder removed")
        .description(format!("Removed reminder **#{idx}**."))
        .field_block("Message", truncate(&removed_content, 1024))
        .field_inline("Was due", format!("<t:{removed_when}:R>"))
        .field_inline("Remaining", (entries.len() - 1).to_string())
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn reply_warn(ctx: &CommandContext, title: &str, message: &str) -> Result<()> {
    let mut embed = Embed2::warning()
        .title(title.to_string())
        .description(message.to_string())
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

/// Parse a leading duration prefix made up of one or more `humantime` tokens.
/// `"1h 30m do laundry"` → `(1h30m, "do laundry")`. Returns `None` if the
/// first whitespace-separated token already fails to parse.
fn consume_duration_prefix(raw: &str) -> Option<(Duration, String)> {
    let mut total = Duration::ZERO;
    let mut consumed = 0usize;
    let mut found_any = false;
    for token in raw.split_whitespace() {
        match parse_duration(token) {
            Ok(d) => {
                // Reject absurd inputs (e.g. `999999999years 999999999years`)
                // rather than panicking in `Duration`'s `Add`. The later
                // `MAX_DURATION` clamp only runs on the *summed* value, so it
                // can't protect this accumulation step.
                total = total.checked_add(d)?;
                let after = &raw[consumed..];
                let local_pos = after.find(token)? + token.len();
                consumed += local_pos;
                // Advance past trailing whitespace by *character* width.
                // `split_whitespace` treats multi-byte Unicode whitespace
                // (U+3000 ideographic space, U+00A0 NBSP, …) as a separator,
                // so stepping one byte at a time would land mid-character and
                // panic with "byte index is not a char boundary".
                while let Some(c) = raw[consumed..].chars().next() {
                    if c.is_whitespace() {
                        consumed += c.len_utf8();
                    } else {
                        break;
                    }
                }
                found_any = true;
            }
            Err(_) => break,
        }
    }
    if !found_any {
        return None;
    }
    Some((total, raw[consumed..].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_prefix() {
        let (d, rest) = consume_duration_prefix("1h 30m do laundry").unwrap();
        assert_eq!(d, Duration::from_secs(90 * 60));
        assert_eq!(rest, "do laundry");
    }

    #[test]
    fn rejects_leading_non_duration() {
        assert!(consume_duration_prefix("hello world").is_none());
    }

    #[test]
    fn handles_multibyte_whitespace_after_token() {
        // U+3000 ideographic space — what a Japanese IME inserts by default.
        // The byte-at-a-time skip used to slice mid-character and panic.
        let (d, rest) = consume_duration_prefix("1h\u{3000}wash dishes").unwrap();
        assert_eq!(d, Duration::from_secs(60 * 60));
        assert_eq!(rest, "wash dishes");
    }

    #[test]
    fn does_not_panic_on_duration_overflow() {
        // Each token parses fine in isolation; the sum overflows `Duration`.
        // Must return None instead of panicking in `Duration::add`.
        assert!(consume_duration_prefix("500000000000years 500000000000years pay rent").is_none());
    }
}
