//! `vndb <query>` — search VNDB by title, or jump directly to a VN by id /
//! URL. Single match goes through as a regular embed; multiple matches are
//! handed off to the paginator so the user can flip between candidates.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use twilight_model::channel::message::Embed;

use tomo_core::error::Result;
use tomo_embed::{Embed2, Embedable};
use tomo_pagination::{Paginator, VecPageSource};
use tomo_requester::vndb::parse_vn_id;
use tomo_requester::VndbVn;

use crate::command::{Command, CommandContext, CommandMeta};
use crate::util::truncate;
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

        let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

        // Direct-lookup shortcut: if the whole query *is* a VN id or URL, fetch
        // that specific VN. Title search treats `v50283` as a literal word and
        // would miss the obvious intent.
        if let Some(id) = parse_id_only(query) {
            return run_direct(&ctx, &id).await;
        }

        let results = match ctx.bot.requester.vndb_search(query, 15).await {
            Ok(v) => v,
            Err(e) => return reply_error(&ctx, &format!("Search failed: `{e}`")).await,
        };

        match results.len() {
            0 => reply_warn(&ctx, &format!("No VN matched `{}`", truncate(query, 200))).await,
            1 => send_single(&ctx, &results[0]).await,
            _ => send_paginated(ctx, results).await,
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

/// Send the single-result embed (no pagination).
async fn send_single(ctx: &CommandContext, vn: &VndbVn) -> Result<()> {
    let nsfw = is_nsfw_channel(ctx);
    let embed = gallery_embed(vn, ctx.author(), nsfw);
    ctx.reply_embed(&embed).await
}

/// Multi-match path: build one embed per VN, hand them to the paginator.
async fn send_paginated(ctx: CommandContext, results: Vec<VndbVn>) -> Result<()> {
    let nsfw = is_nsfw_channel(&ctx);
    let total = results.len();
    let author = ctx.author().cloned();
    let pages: Vec<Embed> = results
        .iter()
        .enumerate()
        .map(|(i, vn)| {
            let mut e = gallery_embed(vn, author.as_ref(), nsfw);
            e = e.footer_with(
                format!("vndb.org · result {} of {total}", i + 1),
                VNDB_ICON,
            );
            e.build_embed()
        })
        .collect();

    let invoker = ctx.author_id();
    let channel_id = ctx.channel_id();
    let source = Arc::new(VecPageSource::new(pages));
    Paginator::new(Arc::clone(&ctx.bot.http), Arc::clone(&ctx.bot.standby), source)
        .run(channel_id, invoker)
        .await
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
