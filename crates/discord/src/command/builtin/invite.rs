use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

/// Permission bitfield embedded in the invite URL. `0` means "no permissions
/// pre-requested" — the server admin picks what to grant.
const INVITE_PERMISSIONS: u64 = 0;

pub struct InviteCommand;

#[async_trait]
impl Command for InviteCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("invite", "Get a link to invite the bot to your server")
                .category("General")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let id = &ctx.bot.identity;
        let app_id = id.application_id.get();
        let url = format!(
            "https://discord.com/api/oauth2/authorize\
             ?client_id={app_id}\
             &scope=bot+applications.commands\
             &permissions={INVITE_PERMISSIONS}"
        );

        let mut embed = Embed2::info()
            .title(format!("Invite {}", id.username))
            .description(format!("[Click here to add **{}** to your server]({url})", id.username))
            .url(url.clone())
            .author_with(id.full_username(), id.avatar_url.clone(), None::<String>)
            .field_inline("Public", if id.bot_public { "yes" } else { "no (owner-only)" })
            .field_inline("Verified", if id.verified { "yes" } else { "no" })
            .field_inline("Scopes", "bot · applications.commands")
            .field_block(
                "How it works",
                "1. Click the link above.\n\
                 2. Pick a server (you need *Manage Server* there).\n\
                 3. Choose which permissions to grant on the consent screen.",
            )
            .footer("Permissions are picked by the server admin on the consent screen.")
            .timestamp_now();

        if let Some(avatar) = id.avatar_url.as_ref() {
            embed = embed.thumbnail(avatar.clone());
        }
        ctx.reply_embed(&embed).await
    }
}
