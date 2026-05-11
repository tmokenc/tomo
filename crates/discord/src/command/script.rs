//! Adapter that exposes a Rhai script as a [`Command`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time;
use tracing::{debug, warn};

use tomo_core::error::{Error, Result};
use tomo_scripting::{ScriptAction, ScriptCommand, ScriptCtx, ScriptManager};

use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};

/// Wraps a [`ScriptCommand`] so it can be put into the [`CommandRegistry`].
pub struct ScriptCommandAdapter {
    pub script: ScriptCommand,
    pub meta: CommandMeta,
    pub manager: Arc<ScriptManager>,
}

impl ScriptCommandAdapter {
    pub fn new(script: ScriptCommand, manager: Arc<ScriptManager>) -> Self {
        // Scripts can declare their own category via `meta.category` so they
        // slot into the help embed alongside built-ins (e.g. `General`,
        // `Utility`) rather than being ghettoised in a `Script` bucket.
        let category = script
            .category
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "General".into());
        let meta = CommandMeta {
            name: script.name.clone(),
            description: script.description.clone(),
            aliases: script.aliases.clone(),
            category,
            slash: script.slash,
            prefix: script.prefix,
            guild_only: script.guild_only,
            owner_only: false,
            options: Vec::new(),
        };
        Self { script, meta, manager }
    }
}

#[async_trait]
impl Command for ScriptCommandAdapter {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let (script_ctx, rx) = ScriptCtx::new(tomo_scripting::ScriptInit {
            channel_id: ctx.channel_id().get(),
            guild_id: ctx.guild_id().map(|g| g.get()),
            user_id: ctx.author_id().get(),
            message_id: ctx.message_id().map(|m| m.get()).unwrap_or(0),
            args: ctx.args().to_string(),
            author_name: ctx
                .author()
                .map(|u| u.name.clone())
                .unwrap_or_default(),
            author_avatar_url: ctx.author_avatar_url(),
            bot_name: ctx.bot.identity.username.clone(),
            bot_avatar_url: ctx.bot.identity.avatar_url.clone(),
            bot_started_at_unix: ctx.bot.started_at.timestamp(),
            now_unix: chrono::Utc::now().timestamp(),
        });

        let actions = time::timeout(
            Duration::from_secs(3),
            self.manager.run_command(&self.script, script_ctx, rx),
        )
        .await
        .map_err(|_| Error::script("script timed out"))??;

        apply_actions(&ctx, actions).await
    }
}

/// Walk the action buffer a script produced and turn each entry into a real
/// Discord API call.
pub async fn apply_actions(ctx: &CommandContext, actions: Vec<ScriptAction>) -> Result<()> {
    for action in actions {
        if let Err(e) = apply_one(ctx, action).await {
            warn!(error = %e, "script action failed");
        }
    }
    Ok(())
}

async fn apply_one(ctx: &CommandContext, action: ScriptAction) -> Result<()> {
    match action {
        ScriptAction::Reply(text) => ctx.reply(&text).await,
        ScriptAction::Send(text) => {
            ctx.bot
                .http
                .create_message(ctx.channel_id())
                .content(&text)
                .await
                .map_err(|e| Error::config(format!("send: {e}")))?;
            Ok(())
        }
        ScriptAction::React(emoji) => {
            if let Some(message_id) = ctx.message_id() {
                let request =
                    twilight_http::request::channel::reaction::RequestReactionType::Unicode {
                        name: &emoji,
                    };
                let _ = ctx
                    .bot
                    .http
                    .create_reaction(ctx.channel_id(), message_id, &request)
                    .await;
            }
            Ok(())
        }
        ScriptAction::DeleteInvocation => {
            if let InvocationSource::Prefix { msg, .. } = &ctx.source {
                let _ = ctx.bot.http.delete_message(msg.channel_id, msg.id).await;
            }
            Ok(())
        }
        ScriptAction::ReplyEmbed(embed) => ctx.reply_embed(&embed).await,
        ScriptAction::SendEmbed(embed) => {
            let built = embed.build();
            ctx.bot
                .http
                .create_message(ctx.channel_id())
                .embeds(std::slice::from_ref(&built))
                .await
                .map_err(|e| Error::config(format!("send_embed: {e}")))?;
            Ok(())
        }
        ScriptAction::Log(msg) => {
            debug!(target: "tomo_script", "{msg}");
            Ok(())
        }
    }
}

