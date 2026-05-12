//! `llm` — owner-only health check for the LLM router.
//!
//! Lists every (provider, model) entry in the configured chain, marks the
//! current primary, and shows any that are presently in cooldown after a
//! 429. Useful for diagnosing "the bot doesn't respond on mention" without
//! tailing logs.

use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct LlmCommand;

#[async_trait]
impl Command for LlmCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("llm", "Show the LLM provider chain + any active cooldowns")
                .aliases(["gemini", "providers"])
                .category("Admin")
                .owner_only()
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let Some(llm) = ctx.bot.llm.as_ref() else {
            let mut embed = Embed2::error()
                .title("LLM")
                .description(
                    "No LLM provider is configured on this bot.\n\
                     Set at least one of: `GEMINI_API_KEY`, `GROQ_API_KEY`, \
                     `CEREBRAS_API_KEY`, `OPENROUTER_API_KEY`, `MISTRAL_API_KEY` \
                     and restart.",
                )
                .timestamp_now();
            if let Some(user) = ctx.author() {
                embed = embed.author_user(user);
            }
            return ctx.reply_embed(&embed).await;
        };

        let entries = llm.router.entries();
        let cooldowns = llm.router.cooldowns();

        // Provider summary — how many entries each provider contributes,
        // useful when the chain interleaves them.
        let mut provider_counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for e in entries {
            *provider_counts.entry(e.provider.name()).or_default() += 1;
        }
        let provider_summary = provider_counts
            .iter()
            .map(|(p, n)| format!("{p}×{n}"))
            .collect::<Vec<_>>()
            .join(", ");

        let chain_body = render_chain(entries, &cooldowns);

        let mut embed = Embed2::success()
            .title(format!("LLM — {} entries in chain", entries.len()))
            .description("Walks top-to-bottom on each mention. Cooled-down entries are skipped.")
            .field_inline("Providers", provider_summary)
            .field_inline("Max output", llm.router.max_output_tokens.to_string())
            .field_inline("Context msgs", llm.router.context_messages.to_string())
            .field_block("Chain", chain_body)
            .field_block("System prompt", llm.router.system_prompt.clone())
            .footer("Admin · tomo>llm")
            .timestamp_now();

        if let Some(user) = ctx.author() {
            embed = embed.author_user(user);
        }
        ctx.reply_embed(&embed).await
    }
}

/// Render the chain as a bulleted list. The first entry gets a ▶ marker
/// (the primary); subsequent entries get a •. Anything currently parked
/// after a 429 shows its remaining cooldown.
fn render_chain(
    entries: &[tomo_llm::RouteEntry],
    cooldowns: &std::collections::HashMap<(String, String), std::time::Duration>,
) -> String {
    if entries.is_empty() {
        return "(chain is empty)".into();
    }
    let mut out = String::with_capacity(entries.len() * 48);
    for (i, e) in entries.iter().enumerate() {
        let marker = if i == 0 { "▶" } else { "•" };
        let key = (e.provider.name().to_string(), e.model.clone());
        match cooldowns.get(&key) {
            Some(d) if d.as_secs() > 0 => out.push_str(&format!(
                "{marker} `{}/{}` — cooled down for **{}s**\n",
                e.provider.name(),
                e.model,
                d.as_secs(),
            )),
            _ => out.push_str(&format!("{marker} `{}/{}` — ready\n", e.provider.name(), e.model)),
        }
    }
    out
}
