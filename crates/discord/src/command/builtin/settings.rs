//! `/settings` — per-guild configuration modal.
//!
//! Slash-only: modals require an interaction token, so prefix invocations
//! have no way to pop one. The framework's permission gate
//! (`required_permissions = MANAGE_GUILD`) makes Discord hide the command
//! from anyone who lacks the permission and re-checks server-side on
//! dispatch. Bot owners bypass the gate everywhere.
//!
//! Modal layout:
//!   * ChannelSelect (writable text channels) — the log destination,
//!     optional (clearing it disables message-log).
//!   * TextInput — server prefix override, empty = global default.
//!
//! On submit we walk the (Label-wrapped) component tree, validate, and
//! persist via [`SettingsStore`].

// `TextInput::label` is deprecated in favour of the newer `Label` wrapper
// component — we wrap below, so this is just `None` here. The deprecation
// warning is otherwise noise.
#![allow(deprecated)]

use std::sync::LazyLock;

use async_trait::async_trait;
use twilight_model::channel::message::component::{
    Component, Label, SelectDefaultValue, SelectMenu, SelectMenuType, TextInput, TextInputStyle,
};
use twilight_model::channel::ChannelType;
use twilight_model::guild::Permissions;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub const MODAL_CUSTOM_ID: &str = "tomo:settings:save";

// Per-input ids — also read back on submit.
pub const INPUT_LOG_CHANNEL: &str = "log_channel";
pub const INPUT_PREFIX: &str = "prefix";

pub struct SettingsCommand;

#[async_trait]
impl Command for SettingsCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("settings", "Open the per-guild settings modal (Manage Guild only).")
                .category("Admin")
                .slash_only()
                // Framework permission gate. Implies `guild_only`. Hides
                // the command from users who don't hold MANAGE_GUILD
                // (Discord-side); the runtime re-checks before dispatch
                // and grants owners regardless.
                .requires_permissions(Permissions::MANAGE_GUILD)
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        // The framework already enforced MANAGE_GUILD or owner bypass.
        // Unwrapping the guild id is safe (the perm gate implies guild_only).
        let guild_id = ctx
            .guild_id()
            .ok_or_else(|| tomo_core::Error::config("settings: missing guild_id"))?;
        let current = ctx.bot.settings.get(guild_id).await;

        // Channel select — restricted to text-postable types so the user
        // can't pick an unwritable channel (voice, forum, etc.). When
        // there's an existing setting we pre-populate the picker.
        let channel_types = vec![
            ChannelType::GuildText,
            ChannelType::GuildAnnouncement,
            ChannelType::AnnouncementThread,
            ChannelType::PublicThread,
            ChannelType::PrivateThread,
        ];
        let log_channel_select = SelectMenu {
            id: None,
            channel_types: Some(channel_types),
            custom_id: INPUT_LOG_CHANNEL.into(),
            default_values: current
                .log_channel
                .map(|c| vec![SelectDefaultValue::Channel(c)]),
            disabled: false,
            kind: SelectMenuType::Channel,
            max_values: Some(1),
            min_values: Some(0),
            options: None,
            placeholder: Some("Pick a log channel (leave empty to disable)".into()),
            required: Some(false),
        };

        let prefix_input = TextInput {
            id: None,
            custom_id: INPUT_PREFIX.into(),
            // The `label` field is deprecated in favour of the newer Label
            // component (we wrap below). Keep `None` here so the wrapper
            // is the canonical source of the label string.
            label: None,
            min_length: Some(0),
            max_length: Some(8),
            placeholder: Some(format!(
                "leave blank to use the default `{}`",
                ctx.bot.config.discord.prefix
            )),
            required: Some(false),
            style: TextInputStyle::Short,
            value: current.prefix.clone(),
        };

        // Label is the modal-v2 wrapper; it carries the visible name +
        // optional description and exactly one child component. Two
        // top-level Labels = two rows in the modal.
        let components = vec![
            Component::Label(Label {
                id: None,
                label: "Log channel".into(),
                description: Some(
                    "Where edit/delete log embeds get posted. Pick from this server's channels."
                        .into(),
                ),
                component: Box::new(Component::SelectMenu(log_channel_select)),
            }),
            Component::Label(Label {
                id: None,
                label: "Command prefix".into(),
                description: Some(
                    "What people type before a prefix command (e.g. `%`). Blank = global default."
                        .into(),
                ),
                component: Box::new(Component::TextInput(prefix_input)),
            }),
        ];

        ctx.respond_modal(MODAL_CUSTOM_ID, "Tomo — server settings", components)
            .await
    }
}

// ---------- Modal submit ----------

use twilight_model::application::interaction::modal::{
    ModalInteractionComponent, ModalInteractionData,
};
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::message::MessageFlags;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::Id;
use twilight_model::id::marker::ChannelMarker;
use twilight_util::builder::InteractionResponseDataBuilder;

use crate::state::Bot;

/// Called from `runtime::on_interaction` when Discord posts a
/// `MODAL_CUSTOM_ID` submit. Re-checks permissions (defence in depth),
/// pulls the values out of the nested component tree, validates,
/// persists, and replies with an ephemeral confirmation embed.
pub async fn handle_submit(bot: Bot, interaction: Interaction, modal: ModalInteractionData) {
    let Some(guild_id) = interaction.guild_id else {
        respond_ephemeral(&bot, &interaction, "Settings only apply inside a guild.").await;
        return;
    };
    let author = interaction
        .author_id()
        .unwrap_or_else(|| Id::new(1));

    // Same hydrate-on-miss path the framework uses — denying for
    // not-in-cache would lock owners out the first time they invoke from
    // a fresh shard.
    let allowed = match crate::permissions::user_has_or_is_owner(
        &bot,
        author,
        guild_id,
        Permissions::MANAGE_GUILD,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            respond_ephemeral(
                &bot,
                &interaction,
                &format!("Couldn't verify your permissions: `{e}`"),
            )
            .await;
            return;
        }
    };
    if !allowed {
        respond_ephemeral(
            &bot,
            &interaction,
            "You need the **Manage Guild** permission to change Tomo's settings.",
        )
        .await;
        return;
    }

    let inputs = collect_modal_values(&modal);

    // ── Channel: walked out of the ChannelSelect's `values`. Empty when
    //    the user cleared the picker. We still verify the channel belongs
    //    to this guild (cache check) so admins can't accidentally point
    //    logging at another server's channel id passed by an old version
    //    of the modal-v1 TextInput. ──
    let log_channel = match inputs.log_channel {
        Some(channel_id) => {
            let belongs = bot
                .cache
                .channel(channel_id)
                .map(|c| c.guild_id == Some(guild_id))
                .unwrap_or(false);
            if !belongs {
                respond_ephemeral(
                    &bot,
                    &interaction,
                    &format!("Channel `{channel_id}` isn't in this guild — settings not saved."),
                )
                .await;
                return;
            }
            Some(channel_id)
        }
        None => None,
    };

    // ── Prefix: empty/whitespace → fall back to default. Reject any
    //    whitespace inside (typing "% " as a prefix is a footgun). ──
    let prefix = {
        let raw = inputs.prefix.unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.chars().any(char::is_whitespace) {
            respond_ephemeral(
                &bot,
                &interaction,
                "Prefix can't contain whitespace — pick something like `%` or `!t.`",
            )
            .await;
            return;
        } else {
            Some(trimmed.to_string())
        }
    };

    let mut next = bot.settings.get(guild_id).await;
    next.log_channel = log_channel;
    next.prefix = prefix;

    if let Err(e) = bot.settings.set(guild_id, next.clone()).await {
        respond_ephemeral(&bot, &interaction, &format!("Saving failed: `{e}`")).await;
        return;
    }

    let mut embed = Embed2::success().title("Settings saved").timestamp_now();
    embed = match next.log_channel {
        Some(c) => embed.field_inline("Log channel", format!("<#{c}>")),
        None => embed.field_inline("Log channel", "*(disabled)*"),
    };
    let global_prefix = &bot.config.discord.prefix;
    embed = match next.prefix.as_deref() {
        Some(p) => embed.field_inline("Prefix", format!("`{p}` (override)")),
        None => embed.field_inline("Prefix", format!("`{global_prefix}` (default)")),
    };
    respond_ephemeral_embed(&bot, &interaction, embed).await;
}

/// Values teased out of the modal submit, with their original shape
/// preserved (the channel is already an `Id<ChannelMarker>` from
/// ChannelSelect's typed `values`; we don't make the user type one in).
#[derive(Default)]
struct SubmittedValues {
    log_channel: Option<Id<ChannelMarker>>,
    prefix: Option<String>,
}

/// Walk the (possibly nested) modal component tree. Handles both the
/// older `ActionRow` and the newer `Label` top-level shapes. ChannelSelect
/// surfaces its picks via `ModalInteractionChannelSelect.values` — we
/// take the first selected channel when there is one, else `None`.
fn collect_modal_values(modal: &ModalInteractionData) -> SubmittedValues {
    use ModalInteractionComponent as M;
    let mut out = SubmittedValues::default();
    fn walk(c: &M, out: &mut SubmittedValues) {
        match c {
            M::TextInput(t) if t.custom_id == INPUT_PREFIX => {
                out.prefix = Some(t.value.clone());
            }
            M::ChannelSelect(s) if s.custom_id == INPUT_LOG_CHANNEL => {
                out.log_channel = s.values.first().copied();
            }
            M::ActionRow(row) => {
                for inner in &row.components {
                    walk(inner, out);
                }
            }
            M::Label(lbl) => walk(&lbl.component, out),
            _ => {}
        }
    }
    for c in &modal.components {
        walk(c, &mut out);
    }
    out
}

async fn respond_ephemeral(bot: &Bot, interaction: &Interaction, text: &str) {
    let data = InteractionResponseDataBuilder::new()
        .content(text.to_string())
        .flags(MessageFlags::EPHEMERAL)
        .build();
    let response = InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(data),
    };
    let client = bot.http.interaction(interaction.application_id);
    if let Err(e) = client
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        tracing::warn!(error = %e, "settings modal: respond_ephemeral failed");
    }
}

async fn respond_ephemeral_embed(bot: &Bot, interaction: &Interaction, embed: Embed2) {
    use tomo_embed::Embedable as _;
    let built = embed.build_embed();
    let data = InteractionResponseDataBuilder::new()
        .embeds([built])
        .flags(MessageFlags::EPHEMERAL)
        .build();
    let response = InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(data),
    };
    let client = bot.http.interaction(interaction.application_id);
    if let Err(e) = client
        .create_response(interaction.id, &interaction.token, &response)
        .await
    {
        tracing::warn!(error = %e, "settings modal: respond_ephemeral_embed failed");
    }
}
