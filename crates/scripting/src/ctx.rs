use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use flume::{Receiver, Sender};
use rhai::{CustomType, TypeBuilder};

use tomo_embed::Embed2;

use crate::embed::ScriptEmbed;

/// Upper bound on the *Discord-API-producing* actions a single script
/// invocation may enqueue. Each becomes a real API call, so without a cap a
/// runaway script (`for i in 0..50000 { ctx.reply("x") }`) — which fits well
/// inside the engine's operation budget — would flood the API for hours.
/// Generous enough for any legitimate script.
const MAX_API_ACTIONS: usize = 25;

/// Separate, looser cap on `Log` actions. These are *not* API calls — the
/// host renders them as `debug!` tracing lines — so they get their own budget
/// and never crowd a script's real reply out of the API budget. The cap still
/// bounds memory against a script that logs in a tight loop.
const MAX_LOG_ACTIONS: usize = 200;

/// Side-effects a script can request. The host drains these after running the
/// script and turns them into Discord API calls.
#[derive(Debug, Clone)]
pub enum ScriptAction {
    Reply(String),
    Send(String),
    React(String),
    DeleteInvocation,
    /// Reply with a fully-built embed.
    ReplyEmbed(Embed2),
    /// Send (not reply) a fresh message with the embed.
    SendEmbed(Embed2),
    /// Free-form log line — useful for debugging scripts.
    Log(String),
}

/// Everything the host knows about an invocation that the script may want to
/// reach for. Used to construct a [`ScriptCtx`] without an exploding positional
/// arg list.
#[derive(Debug, Clone, Default)]
pub struct ScriptInit {
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub user_id: u64,
    pub message_id: u64,
    pub args: String,

    pub author_name: String,
    pub author_avatar_url: Option<String>,

    pub bot_name: String,
    pub bot_avatar_url: Option<String>,
    pub bot_started_at_unix: i64,

    pub now_unix: i64,
}

/// Context object passed into Rhai's `execute(ctx)`.
///
/// The action sender is `Clone`-cheap; Rhai is allowed to clone the value
/// internally without breaking the connection to the host's receiver.
#[derive(Clone)]
pub struct ScriptCtx {
    pub channel_id: i64,
    pub guild_id: i64,
    pub user_id: i64,
    pub message_id: i64,
    pub args: String,

    pub author_name: String,
    pub author_avatar_url: String,

    pub bot_name: String,
    pub bot_avatar_url: String,
    /// Unix timestamp the bot booted on — lets scripts compute uptime.
    pub bot_started_at_unix: i64,
    /// Unix timestamp the bot saw the invoking message.
    pub now_unix: i64,

    actions: Sender<ScriptAction>,
    /// Per-invocation budgets, shared across the cheap `Clone`s Rhai makes.
    budget: Arc<ActionBudget>,
}

/// Per-invocation counters that cap how many actions a script can enqueue,
/// keeping API calls and log lines on independent budgets.
#[derive(Default)]
struct ActionBudget {
    api: AtomicUsize,
    log: AtomicUsize,
}

impl ScriptCtx {
    /// Build a context and its receiving half. The host keeps the receiver to
    /// drain `ScriptAction`s once the script returns.
    pub fn new(init: ScriptInit) -> (Self, Receiver<ScriptAction>) {
        // Unbounded channel; the per-invocation cap is enforced in `push` via
        // `budget` so API actions and log lines have independent limits (a
        // bounded channel would let either kind crowd out the other).
        let (tx, rx) = flume::unbounded();
        let ctx = Self {
            channel_id: init.channel_id as i64,
            guild_id: init.guild_id.map(|g| g as i64).unwrap_or(0),
            user_id: init.user_id as i64,
            message_id: init.message_id as i64,
            args: init.args,
            author_name: init.author_name,
            author_avatar_url: init.author_avatar_url.unwrap_or_default(),
            bot_name: init.bot_name,
            bot_avatar_url: init.bot_avatar_url.unwrap_or_default(),
            bot_started_at_unix: init.bot_started_at_unix,
            now_unix: init.now_unix,
            actions: tx,
            budget: Arc::new(ActionBudget::default()),
        };
        (ctx, rx)
    }

    fn push(&self, action: ScriptAction) {
        // Enforce the per-invocation cap on the budget matching this action's
        // kind. `Log` is host-side tracing, not an API call, so it can't
        // exhaust the API budget and drop a script's real reply.
        let (counter, limit) = match &action {
            ScriptAction::Log(_) => (&self.budget.log, MAX_LOG_ACTIONS),
            _ => (&self.budget.api, MAX_API_ACTIONS),
        };
        if counter.fetch_add(1, Ordering::Relaxed) >= limit {
            return; // budget exhausted — drop silently
        }
        // Send only fails if the receiver was dropped (host discarded the
        // requests); intentional no-op, never blocks the engine.
        let _ = self.actions.send(action);
    }
}

impl CustomType for ScriptCtx {
    fn build(mut builder: TypeBuilder<Self>) {
        builder
            .with_name("Ctx")
            .with_fn("reply", |this: &mut ScriptCtx, msg: &str| {
                this.push(ScriptAction::Reply(msg.to_string()));
            })
            .with_fn("send", |this: &mut ScriptCtx, msg: &str| {
                this.push(ScriptAction::Send(msg.to_string()));
            })
            .with_fn("react", |this: &mut ScriptCtx, emoji: &str| {
                this.push(ScriptAction::React(emoji.to_string()));
            })
            .with_fn("delete_invocation", |this: &mut ScriptCtx| {
                this.push(ScriptAction::DeleteInvocation);
            })
            .with_fn("log", |this: &mut ScriptCtx, msg: &str| {
                this.push(ScriptAction::Log(msg.to_string()));
            })
            // ---- Embed dispatch ----
            // Full-builder form: build a `ScriptEmbed` step by step, then hand
            // it over.
            .with_fn("reply_embed", |this: &mut ScriptCtx, embed: ScriptEmbed| {
                this.push(ScriptAction::ReplyEmbed(embed.into_inner()));
            })
            .with_fn("send_embed", |this: &mut ScriptCtx, embed: ScriptEmbed| {
                this.push(ScriptAction::SendEmbed(embed.into_inner()));
            })
            // Shortcuts kept for terse scripts that just want title/desc/color.
            .with_fn(
                "embed",
                |this: &mut ScriptCtx, title: &str, description: &str, color: i64| {
                    let e = Embed2::new()
                        .title(title.to_string())
                        .description(description.to_string())
                        .color(color as u32);
                    this.push(ScriptAction::ReplyEmbed(e));
                },
            )
            .with_fn(
                "embed_footer",
                |this: &mut ScriptCtx,
                 title: &str,
                 description: &str,
                 color: i64,
                 footer: &str| {
                    let e = Embed2::new()
                        .title(title.to_string())
                        .description(description.to_string())
                        .color(color as u32)
                        .footer(footer.to_string());
                    this.push(ScriptAction::ReplyEmbed(e));
                },
            )
            // ---- Getters ----
            .with_get("channel_id", |this: &mut ScriptCtx| this.channel_id)
            .with_get("guild_id", |this: &mut ScriptCtx| this.guild_id)
            .with_get("user_id", |this: &mut ScriptCtx| this.user_id)
            .with_get("message_id", |this: &mut ScriptCtx| this.message_id)
            .with_get("args", |this: &mut ScriptCtx| this.args.clone())
            .with_get("author_name", |this: &mut ScriptCtx| this.author_name.clone())
            .with_get("author_avatar_url", |this: &mut ScriptCtx| {
                this.author_avatar_url.clone()
            })
            .with_get("bot_name", |this: &mut ScriptCtx| this.bot_name.clone())
            .with_get("bot_avatar_url", |this: &mut ScriptCtx| {
                this.bot_avatar_url.clone()
            })
            .with_get("now_unix", |this: &mut ScriptCtx| this.now_unix)
            .with_get("bot_started_at_unix", |this: &mut ScriptCtx| this.bot_started_at_unix)
            .with_get("uptime_seconds", |this: &mut ScriptCtx| {
                this.now_unix.saturating_sub(this.bot_started_at_unix)
            });
    }
}
