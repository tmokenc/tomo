//! `remind <duration> <text>` / `remind list` / `remind remove <n>` —
//! mirrors `tomoka-rs`'s reminder command. State lives in `tomo-db` under
//! the `reminders` partition; a background task in `DiscordService::run`
//! delivers DMs when each reminder is due.

use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use humantime::parse_duration;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::reminder::{self, Reminder, MAX_DURATION, MAX_PER_USER};

pub struct RemindCommand;

#[async_trait]
impl Command for RemindCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "remind",
                "Set a personal reminder. Subcommands: `list`, `remove <n>`.",
            )
            .aliases(["remindme", "reminder"])
            .category("Utility")
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
        return ctx
            .reply("Usage: `remind <duration> <message>` — e.g. `remind 1h 30m take out trash`.")
            .await;
    }

    let (duration, rest) = match consume_duration_prefix(raw) {
        Some(v) => v,
        None => return ctx.reply(&format!("I couldn't parse a duration from `{raw}`.")).await,
    };

    if duration > MAX_DURATION {
        return ctx
            .reply(&format!(
                "That's more than 90 days away — the cap is `{}`.",
                humantime::format_duration(MAX_DURATION)
            ))
            .await;
    }
    if duration.is_zero() {
        return ctx.reply("Duration must be greater than zero.").await;
    }
    let content = rest.trim();
    if content.is_empty() {
        return ctx.reply("Give me a message to remind you about.").await;
    }
    if content.len() > 1024 {
        return ctx.reply("Reminder text is too long (>1024 chars).").await;
    }

    let user_id = ctx.author_id().get();
    let existing = reminder::list_user(&ctx.bot, user_id).await?;
    if existing.len() >= MAX_PER_USER {
        return ctx
            .reply(&format!("You already have {MAX_PER_USER} pending reminders — clear one first."))
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
    let embed = Embed2::success()
        .title("Reminder set")
        .description(format!("In **{pretty}** — I'll DM you."))
        .field_block("Message", content.to_string())
        .timestamp_unix(when_unix);
    ctx.reply_embed(&embed).await
}

async fn list_handler(ctx: &CommandContext) -> Result<()> {
    let user_id = ctx.author_id().get();
    let entries = reminder::list_user(&ctx.bot, user_id).await?;
    if entries.is_empty() {
        return ctx.reply("You have no pending reminders.").await;
    }

    let mut body = String::new();
    for (i, r) in entries.iter().enumerate() {
        let when = DateTime::from_timestamp(r.when_unix, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| r.when_unix.to_string());
        body.push_str(&format!(
            "**{}.** <t:{}:R> (`{}`)\n— {}\n",
            i + 1,
            r.when_unix,
            when,
            r.content
        ));
    }
    let embed = Embed2::info()
        .title("Your reminders")
        .description(body)
        .footer("Use `remind remove <n>` to cancel one.");
    ctx.reply_embed(&embed).await
}

async fn remove_handler(ctx: &CommandContext, rest: &str) -> Result<()> {
    let idx: usize = match rest.trim().parse() {
        Ok(n) if n >= 1 => n,
        _ => return ctx.reply("Usage: `remind remove <n>` — the index from `remind list`.").await,
    };
    let user_id = ctx.author_id().get();
    let entries = reminder::list_user(&ctx.bot, user_id).await?;
    let Some(target) = entries.get(idx - 1) else {
        return ctx.reply("No reminder at that index.").await;
    };
    reminder::delete(&ctx.bot, target).await?;
    ctx.reply(&format!("Removed reminder #{idx}.")).await
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
                total += d;
                // bump past this token + its trailing whitespace
                let after = &raw[consumed..];
                let local_pos = after.find(token)? + token.len();
                consumed += local_pos;
                while raw[consumed..].starts_with(char::is_whitespace) {
                    consumed += 1;
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

