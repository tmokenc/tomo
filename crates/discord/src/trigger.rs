//! Auto-trigger framework.
//!
//! An auto-trigger fires on every non-bot message that satisfies its
//! [`MatchSpec`]. Triggers can be defined two ways:
//! 1. **Rust** — implement [`Trigger`] directly. Useful when the action needs
//!    rich access to internal state.
//! 2. **Rhai** — drop a script under `<script_dir>/triggers/`. Its `meta()`
//!    must include a `match` map; the loader translates that into a built-in
//!    matcher.
//!
//! The current registry snapshot is held in an [`ArcSwap`] so it can be
//! swapped atomically on hot-reload.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use tokio::time;
use tracing::{debug, warn};
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::channel::Message;
use twilight_model::channel::message::EmojiReactionType;
use twilight_model::gateway::payload::incoming::ReactionAdd;

use tomo_core::error::{Error, Result};
use tomo_scripting::{ScriptCtx, ScriptManager, ScriptTrigger};

use crate::command::script::apply_actions;
use crate::command::{CommandContext, InvocationSource};
use crate::ehentai_view::gallery_embed as ehentai_gallery_embed;
use crate::nhentai_view::gallery_embed as nhentai_gallery_embed;
use crate::vndb_view::gallery_embed as vndb_gallery_embed;
use crate::state::Bot;

#[async_trait]
pub trait Trigger: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn matches(&self, msg: &Message, bot: &Bot) -> bool;
    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()>;
}

pub struct TriggerContext<'a> {
    pub bot: Bot,
    pub message: &'a Message,
}

#[derive(Default)]
pub struct TriggerRegistry {
    builtin: Vec<Arc<dyn Trigger>>,
    /// Triggers built from Rhai scripts. Reload-aware.
    scripts: Vec<ScriptTriggerEntry>,
}

struct ScriptTriggerEntry {
    inner: ScriptTrigger,
    manager: Arc<ScriptManager>,
}

impl TriggerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin(mut self, triggers: Vec<Arc<dyn Trigger>>) -> Self {
        self.builtin = triggers;
        self
    }

    pub fn with_scripts(mut self, manager: Arc<ScriptManager>, triggers: Vec<ScriptTrigger>) -> Self {
        self.scripts = triggers
            .into_iter()
            .map(|inner| ScriptTriggerEntry { inner, manager: Arc::clone(&manager) })
            .collect();
        self
    }

    pub fn iter_builtin(&self) -> impl Iterator<Item = &Arc<dyn Trigger>> {
        self.builtin.iter()
    }

    pub fn iter_script(&self) -> impl Iterator<Item = &ScriptTrigger> {
        self.scripts.iter().map(|e| &e.inner)
    }

    /// Walk every trigger, run anything that matches.
    pub async fn dispatch(&self, bot: &Bot, message: &Message) {
        for trigger in &self.builtin {
            if trigger.matches(message, bot) {
                debug!(name = trigger.name(), "builtin trigger matched");
                let ctx = TriggerContext { bot: Arc::clone(bot), message };
                if let Err(e) = trigger.execute(ctx).await {
                    warn!(name = trigger.name(), error = %e, "trigger failed");
                }
            }
        }

        // Script triggers need a probe — gather it once.
        let probe = build_probe(message, bot);
        for entry in &self.scripts {
            if entry.inner.matcher.matches(&probe) {
                debug!(name = %entry.inner.name, "script trigger matched");
                if let Err(e) = run_script_trigger(bot, &entry.inner, &entry.manager, message).await {
                    warn!(name = %entry.inner.name, error = %e, "script trigger failed");
                }
            }
        }
    }
}

fn build_probe<'a>(message: &'a Message, bot: &Bot) -> tomo_scripting::trigger::MessageProbe<'a> {
    use tomo_scripting::trigger::MessageProbe;
    let has_image = message
        .attachments
        .iter()
        .any(|a| a.content_type.as_deref().map(|t| t.starts_with("image/")).unwrap_or(false));
    let has_attachment = !message.attachments.is_empty();
    let mentions_bot = message.mentions.iter().any(|m| m.id == bot.identity.user_id);
    MessageProbe {
        content: &message.content,
        has_image,
        has_attachment,
        mentions_bot,
    }
}

async fn run_script_trigger(
    bot: &Bot,
    trigger: &ScriptTrigger,
    manager: &Arc<ScriptManager>,
    message: &Message,
) -> Result<()> {
    let author_avatar_url = message.author.avatar.as_ref().map(|hash| {
        let hash = hash.to_string();
        let ext = if hash.starts_with("a_") { "gif" } else { "png" };
        format!(
            "https://cdn.discordapp.com/avatars/{}/{hash}.{ext}?size=256",
            message.author.id,
        )
    });
    let (ctx, rx) = ScriptCtx::new(tomo_scripting::ScriptInit {
        channel_id: message.channel_id.get(),
        guild_id: message.guild_id.map(|g| g.get()),
        user_id: message.author.id.get(),
        message_id: message.id.get(),
        args: message.content.clone(),
        author_name: message.author.name.clone(),
        author_avatar_url,
        bot_name: bot.identity.username.clone(),
        bot_avatar_url: bot.identity.avatar_url.clone(),
        bot_started_at_unix: bot.started_at.timestamp(),
        now_unix: chrono::Utc::now().timestamp(),
    });
    let actions = time::timeout(
        Duration::from_secs(3),
        manager.run_trigger(trigger, ctx, rx),
    )
    .await
    .map_err(|_| Error::script("trigger timed out"))??;

    // Borrow the same machinery commands use to apply actions.
    let cmd_ctx = CommandContext::new(
        Arc::clone(bot),
        InvocationSource::Prefix {
            msg: Box::new(message.clone()),
            args: String::new(),
        },
    );
    apply_actions(&cmd_ctx, actions).await
}

// ---------- Built-in triggers (examples) ----------

/// Quick implementation factory: regex-matched response.
pub struct RegexResponderTrigger {
    name: &'static str,
    regex: Regex,
    response: String,
}

impl RegexResponderTrigger {
    pub fn new(name: &'static str, pattern: &str, response: impl Into<String>) -> Result<Self> {
        Ok(Self {
            name,
            regex: Regex::new(pattern)
                .map_err(|e| Error::config(format!("trigger regex {name}: {e}")))?,
            response: response.into(),
        })
    }
}

#[async_trait]
impl Trigger for RegexResponderTrigger {
    fn name(&self) -> &str { self.name }
    fn matches(&self, msg: &Message, _bot: &Bot) -> bool { self.regex.is_match(&msg.content) }
    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()> {
        ctx.bot
            .http
            .create_message(ctx.message.channel_id)
            .content(&self.response)
            .reply(ctx.message.id)
            .await
            .map_err(|e| Error::config(format!("trigger send: {e}")))?;
        Ok(())
    }
}

/// React with `emoji` on the trigger message, wait for any non-bot user to
/// click it, post the embed, then remove our own reaction. The wait is
/// capped at [`DiscordConfig::gallery_lookup_wait_secs`] seconds; on
/// timeout we also remove our reaction. Returns immediately — runs the
/// whole flow on a spawned task.
fn spawn_react_then_show(
    bot: Bot,
    channel_id: twilight_model::id::Id<twilight_model::id::marker::ChannelMarker>,
    message_id: twilight_model::id::Id<twilight_model::id::marker::MessageMarker>,
    emoji: &'static str,
    embed: twilight_model::channel::message::Embed,
) {
    tokio::spawn(async move {
        let req = RequestReactionType::Unicode { name: emoji };
        if let Err(e) = bot.http.create_reaction(channel_id, message_id, &req).await {
            warn!(error = %e, emoji, "gallery-lookup: failed to add reaction");
            return;
        }

        let wait =
            Duration::from_secs(bot.config.discord.gallery_lookup_wait_secs.max(1));
        let bot_user_id = bot.identity.user_id;
        let standby = Arc::clone(&bot.standby);
        let waiter = standby.wait_for_reaction(message_id, move |r: &ReactionAdd| {
            if r.user_id == bot_user_id {
                return false;
            }
            matches!(&r.emoji, EmojiReactionType::Unicode { name } if name == emoji)
        });

        if let Ok(Ok(_)) = time::timeout(wait, waiter).await {
            if let Err(e) = bot
                .http
                .create_message(channel_id)
                .embeds(std::slice::from_ref(&embed))
                .await
            {
                warn!(error = %e, emoji, "gallery-lookup: failed to send embed");
            }
        }

        // Whether the user clicked or we timed out, take our reaction off so
        // the message doesn't look stuck. Only the bot's own reaction is
        // removed — anyone else's stays.
        let _ = bot
            .http
            .delete_current_user_reaction(channel_id, message_id, &req)
            .await;
    });
}

/// Auto-trigger: when someone posts *only* a number in an NSFW channel, look
/// it up on nhentai. If a gallery exists, react with 🕷️; when a non-bot
/// user clicks that reaction, the bot drops the gallery embed into the
/// channel. After the configured timeout the reaction is removed.
pub struct NhentaiLookupTrigger;

const SPIDER: &str = "🕷️";
const PANDA: &str = "🐼";
const BOOK: &str = "📖";

#[async_trait]
impl Trigger for NhentaiLookupTrigger {
    fn name(&self) -> &str { "nhentai-lookup" }

    fn matches(&self, msg: &Message, bot: &Bot) -> bool {
        if msg.author.bot {
            return false;
        }
        let nsfw = bot
            .cache
            .channel(msg.channel_id)
            .map(|c| c.nsfw.unwrap_or(false))
            .unwrap_or(false);
        if !nsfw {
            return false;
        }
        let trimmed = msg.content.trim();
        !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit())
    }

    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()> {
        let Ok(id) = ctx.message.content.trim().parse::<u64>() else {
            return Ok(());
        };

        // Silently bail on miss / network error — auto-triggers should never
        // be noisy.
        let gallery = match ctx.bot.requester.nhentai(id).await {
            Ok(g) => g,
            Err(_) => return Ok(()),
        };
        let embed = nhentai_gallery_embed(&gallery, Some(&ctx.message.author)).build();
        spawn_react_then_show(
            Arc::clone(&ctx.bot),
            ctx.message.channel_id,
            ctx.message.id,
            SPIDER,
            embed,
        );
        Ok(())
    }
}

/// Auto-trigger: when an NSFW-channel message contains one or more E-Hentai
/// `g/<gid>/<token>/` references (with or without the `e-hentai.org` /
/// `exhentai.org` prefix), look the first one up. Reacts with 🐼 (the
/// "sadpanda"); clicking the panda posts the full embed.
pub struct EhentaiLookupTrigger;

#[async_trait]
impl Trigger for EhentaiLookupTrigger {
    fn name(&self) -> &str { "ehentai-lookup" }

    fn matches(&self, msg: &Message, bot: &Bot) -> bool {
        if msg.author.bot {
            return false;
        }
        let nsfw = bot
            .cache
            .channel(msg.channel_id)
            .map(|c| c.nsfw.unwrap_or(false))
            .unwrap_or(false);
        if !nsfw {
            return false;
        }
        !tomo_requester::ehentai::parse_ids(&msg.content).is_empty()
    }

    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()> {
        let ids = tomo_requester::ehentai::parse_ids(&ctx.message.content);
        let Some(first) = ids.into_iter().next() else { return Ok(()) };

        let galleries = match ctx
            .bot
            .requester
            .ehentai_gmetadata(std::slice::from_ref(&first))
            .await
        {
            Ok(g) => g,
            Err(_) => return Ok(()),
        };
        let Some(gallery) = galleries.into_iter().next() else { return Ok(()) };

        let embed = ehentai_gallery_embed(&gallery, Some(&ctx.message.author)).build();
        spawn_react_then_show(
            Arc::clone(&ctx.bot),
            ctx.message.channel_id,
            ctx.message.id,
            PANDA,
            embed,
        );
        Ok(())
    }
}

/// Auto-trigger: when a message contains a VNDB id (`vNNN`) or a vndb.org
/// URL, look the first one up. Reacts with 📖; clicking the book posts the
/// full embed. Unlike the nhentai/ehentai triggers this fires in *any*
/// channel — VNDB is a general database and the `vN` id pattern is specific
/// enough that we won't fire on ordinary text.
pub struct VndbLookupTrigger;

#[async_trait]
impl Trigger for VndbLookupTrigger {
    fn name(&self) -> &str { "vndb-lookup" }

    fn matches(&self, msg: &Message, _bot: &Bot) -> bool {
        if msg.author.bot {
            return false;
        }
        tomo_requester::vndb::parse_vn_id(&msg.content).is_some()
    }

    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()> {
        let Some(id) = tomo_requester::vndb::parse_vn_id(&ctx.message.content) else {
            return Ok(());
        };

        let vn = match ctx.bot.requester.vndb_by_id(&id).await {
            Ok(Some(v)) => v,
            // Silently ignore on a 404 or transport error — auto-triggers
            // should not turn into channel noise.
            _ => return Ok(()),
        };

        let nsfw = ctx
            .bot
            .cache
            .channel(ctx.message.channel_id)
            .map(|c| c.nsfw.unwrap_or(false))
            .unwrap_or(false);
        let embed = vndb_gallery_embed(&vn, Some(&ctx.message.author), nsfw).build();
        spawn_react_then_show(
            Arc::clone(&ctx.bot),
            ctx.message.channel_id,
            ctx.message.id,
            BOOK,
            embed,
        );
        Ok(())
    }
}

/// Default set the binary wires up unless overridden.
pub fn default_builtin() -> Vec<Arc<dyn Trigger>> {
    let mut v: Vec<Arc<dyn Trigger>> = Vec::new();
    if let Ok(t) = RegexResponderTrigger::new("greet", r"(?i)^hello\s+tomo\b", "Hi there! 👋") {
        v.push(Arc::new(t));
    }
    v.push(Arc::new(NhentaiLookupTrigger));
    v.push(Arc::new(EhentaiLookupTrigger));
    v.push(Arc::new(VndbLookupTrigger));
    v
}
