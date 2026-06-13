use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;
use tracing::warn;
use twilight_http::Client;
use twilight_model::application::interaction::{Interaction, InteractionData};
use twilight_model::channel::message::component::{ActionRow, Button, ButtonStyle};
use twilight_model::channel::message::{Component, Embed, EmojiReactionType, MessageFlags};
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, MessageMarker, UserMarker};
use twilight_standby::Standby;
use twilight_util::builder::InteractionResponseDataBuilder;

use tomo_core::error::{Error, Result};

use crate::page_source::PageSource;

const BTN_FIRST: &str = "tomo:pag:first";
const BTN_PREV:  &str = "tomo:pag:prev";
const BTN_NEXT:  &str = "tomo:pag:next";
const BTN_LAST:  &str = "tomo:pag:last";
const BTN_CLOSE: &str = "tomo:pag:close";

#[derive(Debug, Clone)]
pub struct PaginatorOptions {
    pub idle_timeout: Duration,
    /// Only let the invoking user interact with the paginator. When `false`,
    /// any channel member can flip pages.
    pub author_only: bool,
}

impl Default for PaginatorOptions {
    fn default() -> Self {
        Self { idle_timeout: Duration::from_secs(120), author_only: true }
    }
}

pub struct Paginator<S: PageSource + ?Sized> {
    http: Arc<Client>,
    standby: Arc<Standby>,
    source: Arc<S>,
    opts: PaginatorOptions,
}

impl<S: PageSource + 'static + ?Sized> Paginator<S> {
    pub fn new(http: Arc<Client>, standby: Arc<Standby>, source: Arc<S>) -> Self {
        Self { http, standby, source, opts: PaginatorOptions::default() }
    }

    pub fn with_options(mut self, opts: PaginatorOptions) -> Self {
        self.opts = opts;
        self
    }

    /// Send the first page as a normal channel message and run the paginator
    /// loop. Used by prefix invocations.
    pub async fn run(
        self,
        channel_id: Id<ChannelMarker>,
        invoker: Id<UserMarker>,
    ) -> Result<()> {
        let (current, embed, components) = self.first_page().await?;
        let sent = self
            .http
            .create_message(channel_id)
            .embeds(std::slice::from_ref(&embed))
            .components(&components)
            .await
            .map_err(|e| Error::config(format!("paginator send: {e}")))?
            .model()
            .await
            .map_err(|e| Error::config(format!("paginator decode: {e}")))?;

        self.run_loop(channel_id, sent.id, invoker, current).await
    }

    /// Post the first page as a followup to an already-deferred slash
    /// interaction, then run the paginator loop. The followup *is* the
    /// interaction's response, so the slash command is properly acknowledged.
    /// The caller must have `defer()`-ed the interaction first.
    pub async fn run_interaction(
        self,
        interaction: &Interaction,
        invoker: Id<UserMarker>,
    ) -> Result<()> {
        let (current, embed, components) = self.first_page().await?;
        let client = self.http.interaction(interaction.application_id);
        let sent = client
            .create_followup(&interaction.token)
            .embeds(std::slice::from_ref(&embed))
            .components(&components)
            .await
            .map_err(|e| Error::config(format!("paginator followup: {e}")))?
            .model()
            .await
            .map_err(|e| Error::config(format!("paginator decode: {e}")))?;

        self.run_loop(sent.channel_id, sent.id, invoker, current).await
    }

    /// Compute page 0's embed and button row, erroring if the source is empty.
    async fn first_page(&self) -> Result<(usize, Embed, Vec<Component>)> {
        if self.source.total_pages() == Some(0) {
            return Err(Error::config("paginator: no pages"));
        }
        let embed = self.source.page(0).await?;
        let components = self.build_components(0);
        Ok((0, embed, components))
    }

    /// The shared button-handling loop, driving an already-posted message
    /// `message_id` in `channel_id` until close or idle timeout.
    async fn run_loop(
        &self,
        channel_id: Id<ChannelMarker>,
        message_id: Id<MessageMarker>,
        invoker: Id<UserMarker>,
        mut current: usize,
    ) -> Result<()> {
        let total = self.source.total_pages();

        loop {
            let fut = self.standby.wait_for_component(message_id, |_: &Interaction| true);

            let interaction = match timeout(self.opts.idle_timeout, fut).await {
                Ok(Ok(i)) => i,
                Ok(Err(_canceled)) => break,
                Err(_elapsed) => break,
            };

            let user_id = interaction.author_id().unwrap_or(Id::new(1));
            if self.opts.author_only && user_id != invoker {
                self.deny_other(&interaction).await;
                continue;
            }

            let Some(custom_id) = component_id(&interaction) else { continue };

            if custom_id == BTN_CLOSE {
                self.acknowledge(&interaction, &self.disabled_components(current), None).await;
                return Ok(());
            }

            current = self.advance(&custom_id, current, total);

            let embed = self.source.page(current).await?;
            let components = self.build_components(current);
            self.acknowledge(&interaction, &components, Some(&embed)).await;
        }

        // Idle timeout — disable buttons in place.
        let disabled = self.disabled_components(current);
        if let Err(e) = self
            .http
            .update_message(channel_id, message_id)
            .components(Some(&disabled))
            .await
        {
            warn!(error = %e, "paginator: failed disabling buttons");
        }
        Ok(())
    }

    fn advance(&self, custom_id: &str, current: usize, total: Option<usize>) -> usize {
        match custom_id {
            BTN_FIRST => 0,
            BTN_PREV => current.saturating_sub(1),
            BTN_NEXT => total
                .map(|t| (current + 1).min(t.saturating_sub(1)))
                .unwrap_or(current + 1),
            BTN_LAST => total.map(|t| t.saturating_sub(1)).unwrap_or(current),
            _ => current,
        }
    }

    async fn acknowledge(
        &self,
        interaction: &Interaction,
        components: &[Component],
        embed: Option<&Embed>,
    ) {
        let mut data = InteractionResponseDataBuilder::new().components(components.iter().cloned());
        if let Some(e) = embed {
            data = data.embeds([e.clone()]);
        }
        let response = InteractionResponse {
            kind: InteractionResponseType::UpdateMessage,
            data: Some(data.build()),
        };
        let client = self.http.interaction(interaction.application_id);
        if let Err(e) = client
            .create_response(interaction.id, &interaction.token, &response)
            .await
        {
            warn!(error = %e, "paginator: response failed");
        }
    }

    async fn deny_other(&self, interaction: &Interaction) {
        let data = InteractionResponseDataBuilder::new()
            .content("Sorry — this paginator belongs to someone else.")
            .flags(MessageFlags::EPHEMERAL)
            .build();
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(data),
        };
        let client = self.http.interaction(interaction.application_id);
        let _ = client
            .create_response(interaction.id, &interaction.token, &response)
            .await;
    }

    fn build_components(&self, current: usize) -> Vec<Component> {
        let total = self.source.total_pages();
        let at_start = current == 0;
        let at_end = total.map(|t| current + 1 >= t).unwrap_or(false);

        let row = ActionRow {
            id: None,
            components: vec![
                button(BTN_FIRST, "⏮", ButtonStyle::Secondary, at_start),
                button(BTN_PREV,  "◀", ButtonStyle::Secondary, at_start),
                button(BTN_NEXT,  "▶", ButtonStyle::Secondary, at_end),
                button(BTN_LAST,  "⏭", ButtonStyle::Secondary, at_end || total.is_none()),
                button(BTN_CLOSE, "✕", ButtonStyle::Danger,    false),
            ],
        };
        vec![Component::ActionRow(row)]
    }

    fn disabled_components(&self, current: usize) -> Vec<Component> {
        let mut comps = self.build_components(current);
        for c in &mut comps {
            if let Component::ActionRow(row) = c {
                for inner in &mut row.components {
                    if let Component::Button(b) = inner {
                        b.disabled = true;
                    }
                }
            }
        }
        comps
    }
}

fn button(custom_id: &str, emoji: &str, style: ButtonStyle, disabled: bool) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: Some(custom_id.into()),
        disabled,
        emoji: Some(EmojiReactionType::Unicode { name: emoji.into() }),
        label: None,
        style,
        url: None,
        sku_id: None,
    })
}

fn component_id(interaction: &Interaction) -> Option<String> {
    match interaction.data.as_ref()? {
        InteractionData::MessageComponent(c) => Some(c.custom_id.clone()),
        _ => None,
    }
}
