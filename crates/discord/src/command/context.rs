use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use twilight_model::application::interaction::application_command::CommandData;
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::Message;
use twilight_model::channel::message::MessageFlags;
use twilight_model::http::attachment::Attachment;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker, MessageMarker, UserMarker};
use twilight_model::user::User;
use twilight_util::builder::InteractionResponseDataBuilder;

use tomo_core::error::{Error, Result};
use tomo_embed::Embedable;

use crate::state::Bot;

/// How a command was invoked.
pub enum InvocationSource {
    Prefix {
        msg: Box<Message>,
        /// Arguments after the command name (already trimmed).
        args: String,
    },
    Slash {
        interaction: Box<Interaction>,
        data: Box<CommandData>,
        /// Slash option values flattened into a single string (space-joined
        /// in declared order), so command implementations don't need to
        /// special-case slash vs. prefix.
        args: String,
    },
}

pub struct CommandContext {
    pub bot: Bot,
    pub source: InvocationSource,
    pub started_at: Instant,
    /// Set after the first response is sent. Used by helpers like [`reply`] to
    /// switch between create-response and create-followup for slash commands.
    responded: AtomicBool,
}

impl CommandContext {
    pub fn new(bot: Bot, source: InvocationSource) -> Self {
        Self {
            bot,
            source,
            started_at: Instant::now(),
            responded: AtomicBool::new(false),
        }
    }

    fn take_responded(&self) -> bool {
        self.responded.swap(true, Ordering::SeqCst)
    }

    pub fn channel_id(&self) -> Id<ChannelMarker> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => msg.channel_id,
            InvocationSource::Slash { interaction, .. } => {
                interaction.channel.as_ref().map(|c| c.id).unwrap_or(Id::new(1))
            }
        }
    }

    pub fn guild_id(&self) -> Option<Id<GuildMarker>> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => msg.guild_id,
            InvocationSource::Slash { interaction, .. } => interaction.guild_id,
        }
    }

    pub fn author_id(&self) -> Id<UserMarker> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => msg.author.id,
            InvocationSource::Slash { interaction, .. } => interaction
                .author_id()
                .unwrap_or(Id::new(1)),
        }
    }

    /// The full [`User`] object for the invoking user. For prefix
    /// invocations this is the message author; for slash invocations it's
    /// the member or DM user attached to the interaction. Returns `None`
    /// only for interactions that arrived without an attached user (which
    /// in practice Discord doesn't send).
    pub fn author(&self) -> Option<&User> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => Some(&msg.author),
            InvocationSource::Slash { interaction, .. } => interaction
                .member
                .as_ref()
                .and_then(|m| m.user.as_ref())
                .or(interaction.user.as_ref()),
        }
    }

    /// CDN URL for the invoking user's avatar at 256 px, or `None` if the
    /// user has no custom avatar.
    pub fn author_avatar_url(&self) -> Option<String> {
        let user = self.author()?;
        let hash = user.avatar.as_ref()?;
        let hash_str = hash.to_string();
        let ext = if hash_str.starts_with("a_") { "gif" } else { "png" };
        Some(format!(
            "https://cdn.discordapp.com/avatars/{}/{hash_str}.{ext}?size=256",
            user.id
        ))
    }

    pub fn message_id(&self) -> Option<Id<MessageMarker>> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => Some(msg.id),
            InvocationSource::Slash { interaction, .. } => interaction
                .message
                .as_ref()
                .map(|m| m.id),
        }
    }

    pub fn args(&self) -> &str {
        match &self.source {
            InvocationSource::Prefix { args, .. } | InvocationSource::Slash { args, .. } => {
                args.as_str()
            }
        }
    }

    pub fn is_slash(&self) -> bool {
        matches!(self.source, InvocationSource::Slash { .. })
    }

    /// The underlying interaction for a slash invocation, or `None` for a
    /// prefix one. Lets components that live outside the context (e.g. the
    /// paginator) post through the interaction so the slash command is
    /// actually acknowledged.
    pub fn interaction(&self) -> Option<&Interaction> {
        match &self.source {
            InvocationSource::Slash { interaction, .. } => Some(interaction),
            InvocationSource::Prefix { .. } => None,
        }
    }

    /// Acknowledge a *denied* slash invocation with a private (ephemeral)
    /// message so the user sees why instead of Discord's red "The application
    /// did not respond". A no-op for prefix invocations — denials there stay
    /// silent, matching the bot's long-standing behaviour.
    pub async fn deny_ephemeral(&self, content: &str) -> Result<()> {
        let Some(interaction) = self.interaction() else { return Ok(()) };
        let data = InteractionResponseDataBuilder::new()
            .flags(MessageFlags::EPHEMERAL)
            .content(content.to_string())
            .build();
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(data),
        };
        let client = self.bot.http.interaction(interaction.application_id);
        client
            .create_response(interaction.id, &interaction.token, &response)
            .await
            .map_err(|e| Error::config(format!("deny_ephemeral: {e}")))?;
        self.responded.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Send a plain text response.
    pub async fn reply(&self, content: &str) -> Result<()> {
        match &self.source {
            InvocationSource::Prefix { msg, .. } => {
                self.bot
                    .http
                    .create_message(msg.channel_id)
                    .content(content)
                    .reply(msg.id)
                    .await
                    .map_err(|e| Error::config(format!("send: {e}")))?;
                Ok(())
            }
            InvocationSource::Slash { interaction, .. } => {
                self.respond_text(interaction, content).await
            }
        }
    }

    /// Send an embed response.
    pub async fn reply_embed(&self, embed: &impl Embedable) -> Result<()> {
        let embed = embed.build_embed();
        match &self.source {
            InvocationSource::Prefix { msg, .. } => {
                self.bot
                    .http
                    .create_message(msg.channel_id)
                    .embeds(std::slice::from_ref(&embed))
                    .reply(msg.id)
                    .await
                    .map_err(|e| Error::config(format!("send: {e}")))?;
                Ok(())
            }
            InvocationSource::Slash { interaction, .. } => {
                self.respond_embed(interaction, embed).await
            }
        }
    }

    async fn respond_text(&self, interaction: &Interaction, content: &str) -> Result<()> {
        if self.take_responded() {
            // Follow up.
            let client = self.bot.http.interaction(interaction.application_id);
            client
                .create_followup(&interaction.token)
                .content(content)
                .await
                .map_err(|e| Error::config(format!("followup: {e}")))?;
            return Ok(());
        }
        let data = InteractionResponseDataBuilder::new()
            .content(content.to_string())
            .build();
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(data),
        };
        let client = self.bot.http.interaction(interaction.application_id);
        client
            .create_response(interaction.id, &interaction.token, &response)
            .await
            .map_err(|e| Error::config(format!("respond: {e}")))?;
        Ok(())
    }

    async fn respond_embed(
        &self,
        interaction: &Interaction,
        embed: twilight_model::channel::message::Embed,
    ) -> Result<()> {
        if self.take_responded() {
            let client = self.bot.http.interaction(interaction.application_id);
            client
                .create_followup(&interaction.token)
                .embeds(std::slice::from_ref(&embed))
                .await
                .map_err(|e| Error::config(format!("followup: {e}")))?;
            return Ok(());
        }
        let data = InteractionResponseDataBuilder::new()
            .embeds([embed])
            .build();
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(data),
        };
        let client = self.bot.http.interaction(interaction.application_id);
        client
            .create_response(interaction.id, &interaction.token, &response)
            .await
            .map_err(|e| Error::config(format!("respond: {e}")))?;
        Ok(())
    }

    /// Send an embed plus file attachments. Routes the same way as
    /// [`Self::reply_embed`]: a real reply for prefix, an interaction
    /// response/followup for slash (so the interaction is acknowledged
    /// instead of left hanging). Used by commands like `qr` that ship a PNG.
    pub async fn reply_embed_with_attachments(
        &self,
        embed: &impl Embedable,
        attachments: &[Attachment],
    ) -> Result<()> {
        let embed = embed.build_embed();
        match &self.source {
            InvocationSource::Prefix { msg, .. } => {
                self.bot
                    .http
                    .create_message(msg.channel_id)
                    .embeds(std::slice::from_ref(&embed))
                    .attachments(attachments)
                    .reply(msg.id)
                    .await
                    .map_err(|e| Error::config(format!("send: {e}")))?;
                Ok(())
            }
            InvocationSource::Slash { interaction, .. } => {
                let client = self.bot.http.interaction(interaction.application_id);
                if self.take_responded() {
                    client
                        .create_followup(&interaction.token)
                        .embeds(std::slice::from_ref(&embed))
                        .attachments(attachments)
                        .await
                        .map_err(|e| Error::config(format!("followup: {e}")))?;
                    return Ok(());
                }
                let data = InteractionResponseDataBuilder::new()
                    .embeds([embed])
                    .attachments(attachments.iter().cloned())
                    .build();
                let response = InteractionResponse {
                    kind: InteractionResponseType::ChannelMessageWithSource,
                    data: Some(data),
                };
                client
                    .create_response(interaction.id, &interaction.token, &response)
                    .await
                    .map_err(|e| Error::config(format!("respond: {e}")))?;
                Ok(())
            }
        }
    }

    /// Send a `Modal` interaction response. Slash-only — for prefix
    /// invocations this returns `Ok(())` after a debug log (modals require
    /// an interaction token; a plain message can't pop one).
    ///
    /// `components` must be a Vec of `ActionRow`s, each carrying exactly
    /// one `TextInput` (Discord's modal constraint).
    pub async fn respond_modal(
        &self,
        custom_id: &str,
        title: &str,
        components: Vec<twilight_model::channel::message::Component>,
    ) -> Result<()> {
        let InvocationSource::Slash { interaction, .. } = &self.source else {
            tracing::debug!("respond_modal called from a prefix invocation — ignoring");
            return Ok(());
        };
        let data = InteractionResponseDataBuilder::new()
            .custom_id(custom_id.to_string())
            .title(title.to_string())
            .components(components)
            .build();
        let response = InteractionResponse {
            kind: InteractionResponseType::Modal,
            data: Some(data),
        };
        let client = self.bot.http.interaction(interaction.application_id);
        client
            .create_response(interaction.id, &interaction.token, &response)
            .await
            .map_err(|e| Error::config(format!("respond_modal: {e}")))?;
        self.responded.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Defer a slash command response — useful when the command does slow work.
    pub async fn defer(&self) -> Result<()> {
        let InvocationSource::Slash { interaction, .. } = &self.source else { return Ok(()); };
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: None,
        };
        let client = self.bot.http.interaction(interaction.application_id);
        client
            .create_response(interaction.id, &interaction.token, &response)
            .await
            .map_err(|e| Error::config(format!("defer: {e}")))?;
        self.responded.store(true, Ordering::SeqCst);
        Ok(())
    }
}
