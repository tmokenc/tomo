//! `anime <query>` and `manga <query>` — AniList lookups.
//!
//! For multi-match queries the paginator fetches the full record for each
//! entry **lazily**, only when the user reaches that page. The initial
//! search call is a cheap brief query that returns just `id`+`title` per
//! result. This keeps "/anime kim" snappy (200 ms) instead of paying for
//! 25 full media bodies up front.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use tomo_pagination::{PageSource, Paginator};
use tomo_requester::{AniListBrief, AniListType};
use twilight_model::channel::message::Embed;
use twilight_model::user::User;

use tomo_core::error::Result;
use tomo_embed::{Embed2, Embedable};

use crate::command::{Command, CommandContext, CommandMeta};
use crate::state::Bot;
use crate::util::truncate;
use crate::anilist_view::{brief_embed, media_embed};

const ANILIST_ICON: &str = "https://anilist.co/img/icons/favicon-32x32.png";

pub struct AnimeCommand;
pub struct MangaCommand;

#[async_trait]
impl Command for AnimeCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("anime", "Search AniList for an anime by title.")
                .aliases(["ani", "al"])
                .category("Search")
                .string_option("query", "Title to search for.", true)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        run(ctx, AniListType::Anime).await
    }
}

#[async_trait]
impl Command for MangaCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("manga", "Search AniList for a manga by title.")
                .category("Search")
                .string_option("query", "Title to search for.", true)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        run(ctx, AniListType::Manga).await
    }
}

async fn run(ctx: CommandContext, media_type: AniListType) -> Result<()> {
    let query = ctx.args().trim();
    if query.is_empty() {
        return reply_warn(&ctx, media_type, "Give me a title to search for.").await;
    }

    // Slash: acknowledge before the AniList round-trip so a slow search can't
    // blow Discord's 3s window. Later responses go out as followups.
    if ctx.is_slash() {
        let _ = ctx.defer().await;
    }
    let _ = ctx.bot.http.create_typing_trigger(ctx.channel_id()).await;

    // Brief search — just IDs + titles. The paginator hydrates each entry
    // on demand via `anilist_by_id` when the user lands on it.
    let briefs = match ctx
        .bot
        .requester
        .anilist_search_brief(query, media_type, 15)
        .await
    {
        Ok(v) => v,
        Err(e) => return reply_error(&ctx, media_type, &format!("Search failed: `{e}`")).await,
    };

    match briefs.len() {
        0 => {
            reply_warn(
                &ctx,
                media_type,
                &format!("No {} matched `{}`", media_type.label(), truncate(query, 200)),
            )
            .await
        }
        1 => send_single(&ctx, briefs[0].id, media_type).await,
        _ => send_paginated(ctx, briefs, media_type).await,
    }
}

async fn send_single(ctx: &CommandContext, id: u64, media_type: AniListType) -> Result<()> {
    let media = match ctx.bot.requester.anilist_by_id(id, media_type).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            return reply_warn(
                ctx,
                media_type,
                &format!("No {} with id `{id}`.", media_type.label()),
            )
            .await
        }
        Err(e) => return reply_error(ctx, media_type, &format!("Lookup failed: `{e}`")).await,
    };
    let nsfw = is_nsfw_channel(ctx);
    ctx.reply_embed(&media_embed(&media, ctx.author(), nsfw)).await
}

async fn send_paginated(
    ctx: CommandContext,
    briefs: Vec<AniListBrief>,
    media_type: AniListType,
) -> Result<()> {
    let nsfw = is_nsfw_channel(&ctx);
    let invoker = ctx.author().cloned();
    let total = briefs.len();
    let source = Arc::new(LazyAniListPages::new(
        Arc::clone(&ctx.bot),
        briefs,
        media_type,
        invoker,
        nsfw,
        total,
    ));
    let invoker_id = ctx.author_id();
    let channel_id = ctx.channel_id();
    let paginator = Paginator::new(Arc::clone(&ctx.bot.http), Arc::clone(&ctx.bot.standby), source);
    // Slash already deferred in `run`; post pages as the interaction's
    // followup. Prefix posts a normal channel message.
    if let Some(interaction) = ctx.interaction() {
        paginator.run_interaction(interaction, invoker_id).await
    } else {
        paginator.run(channel_id, invoker_id).await
    }
}

/// Page source that holds the search briefs and fetches the full media body
/// per-page on first access. Subsequent visits to the same page hit the
/// embed cache and skip the network entirely.
struct LazyAniListPages {
    bot: Bot,
    briefs: Vec<AniListBrief>,
    media_type: AniListType,
    invoker: Option<User>,
    nsfw_channel: bool,
    total: usize,
    /// `index -> rendered embed`. Behind a `Mutex` because [`PageSource`] is
    /// `&self`-async — paginator handles only one navigation at a time, but
    /// the lock keeps the contract honest.
    cache: Mutex<HashMap<usize, Embed>>,
}

impl LazyAniListPages {
    fn new(
        bot: Bot,
        briefs: Vec<AniListBrief>,
        media_type: AniListType,
        invoker: Option<User>,
        nsfw_channel: bool,
        total: usize,
    ) -> Self {
        Self {
            bot,
            briefs,
            media_type,
            invoker,
            nsfw_channel,
            total,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl PageSource for LazyAniListPages {
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
            return Ok(fallback_embed("Out of range.").build_embed());
        };

        // Show the brief immediately to give the user a "loading" state, then
        // try to fetch the full record. On any failure we fall back to the
        // brief embed so the paginator stays usable.
        let full = self
            .bot
            .requester
            .anilist_by_id(brief.id, self.media_type)
            .await;

        let embed = match full {
            Ok(Some(media)) => {
                let mut e = media_embed(&media, self.invoker.as_ref(), self.nsfw_channel);
                e = e.footer_with(
                    format!("anilist.co · result {} of {}", index + 1, self.total),
                    ANILIST_ICON,
                );
                e.build_embed()
            }
            Ok(None) => {
                let mut e = brief_embed(brief, self.media_type, self.invoker.as_ref(), index, self.total);
                e = e
                    .description(format!(
                        "Couldn't load full details for `{}` (entry removed on AniList).",
                        brief.id
                    ))
                    .footer_with(
                        format!("anilist.co · result {} of {} · brief-only", index + 1, self.total),
                        ANILIST_ICON,
                    );
                e.build_embed()
            }
            Err(e) => {
                let mut em = brief_embed(brief, self.media_type, self.invoker.as_ref(), index, self.total);
                em = em
                    .description(format!("Lookup failed: `{e}`."))
                    .footer_with(
                        format!("anilist.co · result {} of {} · cache miss", index + 1, self.total),
                        ANILIST_ICON,
                    );
                em.build_embed()
            }
        };

        if let Ok(mut map) = self.cache.lock() {
            map.insert(index, embed.clone());
        }
        Ok(embed)
    }
}

fn fallback_embed(message: &str) -> Embed2 {
    Embed2::error()
        .title("AniList")
        .description(message.to_string())
        .footer_with("anilist.co", ANILIST_ICON)
        .timestamp_now()
}

fn is_nsfw_channel(ctx: &CommandContext) -> bool {
    ctx.bot
        .cache
        .channel(ctx.channel_id())
        .map(|c| c.nsfw.unwrap_or(false))
        .unwrap_or(false)
}

async fn reply_warn(
    ctx: &CommandContext,
    media_type: AniListType,
    message: &str,
) -> Result<()> {
    let mut embed = Embed2::warning()
        .title(format!("AniList — {}", media_type.label()))
        .description(message.to_string())
        .footer_with("anilist.co", ANILIST_ICON)
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}

async fn reply_error(
    ctx: &CommandContext,
    media_type: AniListType,
    message: &str,
) -> Result<()> {
    let mut embed = Embed2::error()
        .title(format!("AniList — {}", media_type.label()))
        .description(message.to_string())
        .footer_with("anilist.co", ANILIST_ICON)
        .timestamp_now();
    if let Some(user) = ctx.author() {
        embed = embed.author_user(user);
    }
    ctx.reply_embed(&embed).await
}
