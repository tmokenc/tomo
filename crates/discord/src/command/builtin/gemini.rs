//! `gemini` — owner-only health check for the Gemini API connection.
//!
//! Pings `GET /v1beta/models/<primary>` and reports the result + the rest of
//! the fallback chain (with any current cooldowns) as an embed. Useful for
//! diagnosing "the bot doesn't respond on mention" without tailing logs.

use std::sync::LazyLock;
use std::time::Instant;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct GeminiCommand;

#[async_trait]
impl Command for GeminiCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("gemini", "Check that the Gemini API is reachable")
                .category("Admin")
                .owner_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let Some(gemini) = ctx.bot.gemini.as_ref() else {
            let mut embed = Embed2::error()
                .title("Gemini")
                .description(
                    "Gemini is **not configured** on this bot.\n\
                     Set `GEMINI_API_KEY` (and optionally `GEMINI_MODEL`) and restart.",
                )
                .timestamp_now();
            if let Some(user) = ctx.author() {
                embed = embed.author_user(user);
            }
            return ctx.reply_embed(&embed).await;
        };

        let cfg = gemini.client.config().clone();
        let cooldowns = gemini.client.cooldowns();
        let start = Instant::now();
        let result = gemini.client.health_check().await;
        let elapsed = start.elapsed();

        let chain_body = chain_status(&cfg.models, &cooldowns);

        let mut embed = match result {
            Ok(info) => Embed2::success()
                .title("Gemini — OK")
                .description("Primary model reachable. Fallback chain below.")
                .field_inline("Primary", info.name)
                .field_inline("Display name", info.display_name)
                .field_inline("Latency", format!("{elapsed:?}"))
                .field_inline("Input tokens", info.input_token_limit.to_string())
                .field_inline("Output tokens (limit)", info.output_token_limit.to_string())
                .field_inline("Max output (config)", cfg.max_output_tokens.to_string())
                .field_inline("Context msgs", cfg.context_messages.to_string())
                .field_inline("Rate limit / user", format!("{}/min", cfg.rate_limit_per_minute))
                .field_block("Fallback chain", chain_body)
                .field_block("System prompt", cfg.system_prompt),
            Err(e) => Embed2::error()
                .title("Gemini — FAILED")
                .description(format!("`{e}`"))
                .field_inline("Primary (configured)", cfg.primary_model().to_string())
                .field_inline("Latency", format!("{elapsed:?}"))
                .field_block("Fallback chain", chain_body)
                .field_block(
                    "Things to check",
                    "• `GEMINI_API_KEY` is set and not expired\n\
                     • Every model in `GEMINI_MODEL` (comma-separated) is one the key can see\n\
                     • Network egress to `generativelanguage.googleapis.com` is allowed\n\
                     • The key has the *Generative Language API* enabled in Google Cloud",
                ),
        };

        embed = embed.footer("Admin · /gemini").timestamp_now();
        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}

/// Render the fallback chain as a bulleted list. Each entry is either
/// `• gemini-2.5-flash` (eligible) or `• gemini-2.5-flash — cooled down for 42s`
/// when a 429 has parked it.
fn chain_status(
    models: &[String],
    cooldowns: &std::collections::HashMap<String, std::time::Duration>,
) -> String {
    let mut out = String::with_capacity(models.len() * 32);
    for (i, m) in models.iter().enumerate() {
        let marker = if i == 0 { "▶" } else { "•" };
        match cooldowns.get(m) {
            Some(d) if d.as_secs() > 0 => out.push_str(&format!(
                "{marker} `{m}` — cooled down for **{}s**\n",
                d.as_secs(),
            )),
            _ => out.push_str(&format!("{marker} `{m}` — ready\n")),
        }
    }
    if out.is_empty() {
        "(no models configured)".into()
    } else {
        out
    }
}
