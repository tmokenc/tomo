//! `vndb <query>` — search VNDB by title, or jump directly to a VN by id /
//! URL. Single match goes through as a regular embed; multiple matches are
//! handed off to the paginator so the user can flip between candidates.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use tomo_pagination::{PageSource, Paginator};
use twilight_model::channel::message::Embed;
use twilight_model::user::User;

use tomo_core::error::Result;
use tomo_embed::{Embed2, Embedable};
use tomo_requester::vndb::parse_vn_id;
use tomo_requester::{VndbBrief, VndbVn};

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
use crate::state::Bot;
use crate::util::{message_only_links_to, suppress_embeds, truncate};
use crate::vndb_view::gallery_embed;

const VNDB_ICON: &str = "https://s.vndb.org/s/angel.png";

pub struct VndbCommand;

#[async_trait]
impl Command for VndbCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new(
                "vndb",
                "Search VNDB by title, or jump straight to a VN by id (e.g. `v50283`) or URL.",
            )
            .aliases(["vn"])
            .category("Search")
            .string_option(
                "query",
                "Title to search for, or a `vNNN` id / vndb.org URL for a direct lookup.",
                true,
            )
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let query = ctx.args().trim();
        if query.is_empty() {
            return reply_warn(&ctx, "Give me a title, a `vNNN` id, or a vndb.org URL.").await;
        }

        // Slash: acknowledge before any upstream call so a slow VNDB
        // round-trip can't blow Discord's 3s window. Every later response
        // (single embed, pagination, errors) then goes out as a followup.
        if ctx.is_slash() {
            let _ = ctx.defer().await;
        }
        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        // Direct-lookup shortcut: if the whole query *is* a VN id or URL, fetch
        // that specific VN. Title search treats `v50283` as a literal word and
        // would miss the obvious intent.
        if let Some(id) = parse_id_only(query) {
            return run_direct(&ctx, &id).await;
        }

        // Brief search — `id`/`title`/`alttitle`/`released` only. Full
        // records are fetched lazily by the paginator per page so the user
        // doesn't pay for 15 descriptions+tag lists upfront.
        let briefs = match ctx.bot.requester.vndb_search_brief(query, 15).await {
            Ok(v) => v,
            Err(e) => return reply_error(&ctx, &format!("Search failed: `{e}`")).await,
        };

        match briefs.len() {
            0 => reply_warn(&ctx, &format!("No VN matched `{}`", truncate(query, 200))).await,
            1 => run_direct(&ctx, &briefs[0].id.clone()).await,
            _ => send_paginated(ctx, briefs).await,
        }
    }
}

/// Look up a single VN by id and post its embed.
async fn run_direct(ctx: &CommandContext, id: &str) -> Result<()> {
    match ctx.bot.requester.vndb_by_id(id).await {
        Ok(Some(vn)) => send_single(ctx, &vn).await,
        Ok(None) => reply_warn(ctx, &format!("No VN with id `{id}`.")).await,
        Err(e) => reply_error(ctx, &format!("Lookup failed: `{e}`")).await,
    }
}

/// Send the single-result embed (no pagination). When the user invoked
/// us by pasting a vndb.org URL in a prefix command, suppress Discord's
/// auto-embed on their message so the channel doesn't show both our
/// rich embed and Discord's plain link preview.
async fn send_single(ctx: &CommandContext, vn: &VndbVn) -> Result<()> {
    let nsfw = is_nsfw_channel(ctx);
    let embed = gallery_embed(vn, ctx.author(), nsfw);
    ctx.reply_embed(&embed).await?;
    suppress_source_if_url(ctx).await;
    Ok(())
}

/// Strip the source message's auto-embed when the invocation was a
/// prefix command and the original content contained a URL.
async fn suppress_source_if_url(ctx: &CommandContext) {
    if let InvocationSource::Prefix { msg, .. } = &ctx.source {
        if message_only_links_to(&msg.content, &["vndb.org"]) {
            suppress_embeds(&ctx.bot, msg.channel_id, msg.id).await;
        }
    }
}

/// Multi-match path: hand the briefs to a lazy paginator that fetches each
/// VN's full body the moment the user lands on its page. Already-viewed
/// pages are cached, so flipping back is free.
async fn send_paginated(ctx: CommandContext, briefs: Vec<VndbBrief>) -> Result<()> {
    let nsfw = is_nsfw_channel(&ctx);
    let invoker = ctx.author().cloned();
    let total = briefs.len();
    let source = Arc::new(LazyVndbPages::new(
        Arc::clone(&ctx.bot),
        briefs,
        invoker,
        nsfw,
        total,
    ));
    let invoker_id = ctx.author_id();
    let channel_id = ctx.channel_id();
    let paginator = Paginator::new(Arc::clone(&ctx.bot.http), Arc::clone(&ctx.bot.standby), source);
    // Slash already deferred in `execute`; post pages as the interaction's
    // followup. Prefix posts a normal channel message.
    if let Some(interaction) = ctx.interaction() {
        paginator.run_interaction(interaction, invoker_id).await
    } else {
        paginator.run(channel_id, invoker_id).await
    }
}

/// Page source that holds the search briefs and fetches the full VN per
/// page on first access. Mirrors `LazyAniListPages` in spirit; the two
/// don't share concrete code because the API shapes diverge.
struct LazyVndbPages {
    bot: Bot,
    briefs: Vec<VndbBrief>,
    invoker: Option<User>,
    nsfw_channel: bool,
    total: usize,
    cache: Mutex<HashMap<usize, Embed>>,
}

impl LazyVndbPages {
    fn new(
        bot: Bot,
        briefs: Vec<VndbBrief>,
        invoker: Option<User>,
        nsfw_channel: bool,
        total: usize,
    ) -> Self {
        Self {
            bot,
            briefs,
            invoker,
            nsfw_channel,
            total,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl PageSource for LazyVndbPages {
    fn total_pages(&self) -> Option<usize> {
        Some(self.total)
    }

    async fn page(&self, index: usize) -> Result<Embed> {
        if let Ok(map) = self.cache.lock() {
            if let Some(embed) = map.get(&index) {
                return Ok(embed.clone());
            }
        }

        let Some(brief) = self.briefs.get(index) else {
            return Ok(Embed2::error()
                .title("VNDB")
                .description("Out of range.")
                .footer_with("vndb.org", VNDB_ICON)
                .build_embed());
        };

        let full = self.bot.requester.vndb_by_id(&brief.id).await;
        let embed = match full {
            Ok(Some(vn)) => {
                let mut e = gallery_embed(&vn, self.invoker.as_ref(), self.nsfw_channel);
                e = e.footer_with(
                    format!("vndb.org · result {} of {}", index + 1, self.total),
                    VNDB_ICON,
                );
                e.build_embed()
            }
            Ok(None) => brief_fallback(brief, self.invoker.as_ref(), index, self.total, "removed on VNDB"),
            Err(e) => brief_fallback(
                brief,
                self.invoker.as_ref(),
                index,
                self.total,
                &format!("Lookup failed: `{e}`"),
            ),
        };

        if let Ok(mut map) = self.cache.lock() {
            map.insert(index, embed.clone());
        }
        Ok(embed)
    }
}

/// Show what we know from the search brief plus a reason the full fetch
/// couldn't fill in the rest. Keeps the paginator usable instead of
/// erroring out for one bad row.
fn brief_fallback(
    brief: &VndbBrief,
    invoker: Option<&User>,
    index: usize,
    total: usize,
    reason: &str,
) -> Embed {
    let mut e = Embed2::warning()
        .title(truncate(brief.display_title(), 256))
        .url(brief.url())
        .description(format!(
            "Couldn't load full details ({reason})."
        ))
        .field_inline("ID", format!("`{}`", brief.id))
        .footer_with(
            format!("vndb.org · result {} of {} · brief-only", index + 1, total),
            VNDB_ICON,
        );
    if let Some(rel) = brief.released.as_ref().filter(|s| !s.is_empty()) {
        e = e.field_inline("Released", rel.clone());
    }
    if let Some(user) = invoker {
        e = e.author_user(user);
    }
    e.build_embed()
}

/// Detect "the user typed nothing but an id or a vndb URL". Whitespace is
/// trimmed; trailing slashes/punctuation tolerated.
fn parse_id_only(query: &str) -> Option<String> {
    let trimmed = query.trim().trim_end_matches('/');
    let extracted = parse_vn_id(trimmed)?;
    // The query must be JUST the id (or `vndb.org/<id>` style URL) — not a
    // sentence that happens to contain `v123`. We accept the query if either:
    //   - it is exactly the id (case-insensitive), or
    //   - it ends with `/<id>` after stripping the trailing slash.
    let lower = trimmed.to_ascii_lowercase();
    if lower == extracted {
        return Some(extracted);
    }
    if lower.ends_with(&format!("/{extracted}")) {
        return Some(extracted);
    }
    None
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}

async fn reply_warn(ctx: &CommandContext, message: &str) -> Result<()> {
    let mut embed = Embed2::warning()
        .title("VNDB")
        .description(message.to_string())
        .footer_with("vndb.org", VNDB_ICON)
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn reply_error(ctx: &CommandContext, message: &str) -> Result<()> {
    let mut embed = Embed2::error()
        .title("VNDB")
        .description(message.to_string())
        .footer_with("vndb.org", VNDB_ICON)
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

#[cfg(test)]
mod tests {
    use super::parse_id_only;

    #[test]
    fn id_only_accepts_bare_id() {
        assert_eq!(parse_id_only("v50283").as_deref(), Some("v50283"));
        assert_eq!(parse_id_only("  V42 ").as_deref(), Some("v42"));
    }

    #[test]
    fn id_only_accepts_vndb_url() {
        assert_eq!(
            parse_id_only("https://vndb.org/v50283").as_deref(),
            Some("v50283"),
        );
        assert_eq!(
            parse_id_only("https://vndb.org/v50283/").as_deref(),
            Some("v50283"),
        );
    }

    #[test]
    fn id_only_rejects_id_in_sentence() {
        assert_eq!(parse_id_only("check this v50283 game"), None);
    }
}
