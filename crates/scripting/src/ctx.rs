use flume::{Receiver, Sender};
use rhai::{CustomType, TypeBuilder};

use tomo_embed::Embed2;

use crate::embed::ScriptEmbed;

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
    pub bot_name: String,
    /// Unix timestamp the bot booted on — lets scripts compute uptime.
    pub bot_started_at_unix: i64,
    /// Unix timestamp the bot saw the invoking message.
    pub now_unix: i64,
    actions: Sender<ScriptAction>,
}

impl ScriptCtx {
    /// Build a context and its receiving half. The host keeps the receiver to
    /// drain `ScriptAction`s once the script returns.
    #[allow(clippy::too_many_arguments)]
    pub fn channel(
        channel_id: u64,
        guild_id: Option<u64>,
        user_id: u64,
        message_id: u64,
        args: String,
        bot_name: String,
        bot_started_at_unix: i64,
        now_unix: i64,
    ) -> (Self, Receiver<ScriptAction>) {
        let (tx, rx) = flume::unbounded();
        let ctx = Self {
            channel_id: channel_id as i64,
            guild_id: guild_id.map(|g| g as i64).unwrap_or(0),
            user_id: user_id as i64,
            message_id: message_id as i64,
            args,
            bot_name,
            bot_started_at_unix,
            now_unix,
            actions: tx,
        };
        (ctx, rx)
    }

    fn push(&self, action: ScriptAction) {
        // Unbounded send only fails on a closed channel; that's only possible
        // if the host dropped the receiver, in which case the script's
        // requests are intentionally being discarded.
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
            // Full-builder form: build a `ScriptEmbed` step by step, then
            // hand it over.
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
            .with_get("bot_name", |this: &mut ScriptCtx| this.bot_name.clone())
            .with_get("now_unix", |this: &mut ScriptCtx| this.now_unix)
            .with_get("bot_started_at_unix", |this: &mut ScriptCtx| this.bot_started_at_unix)
            .with_get("uptime_seconds", |this: &mut ScriptCtx| {
                this.now_unix.saturating_sub(this.bot_started_at_unix)
            });
    }
}
