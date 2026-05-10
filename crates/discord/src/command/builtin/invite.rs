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
        let app_id = ctx.bot.identity.application_id.get();
        let url = format!(
            "https://discord.com/api/oauth2/authorize\
             ?client_id={app_id}\
             &scope=bot+applications.commands\
             &permissions={INVITE_PERMISSIONS}"
        );
        let embed = Embed2::info()
            .title("Invite me")
            .description(format!("[Click here to add **{}**]({url})", ctx.bot.identity.username))
            .footer("Pick the permissions you trust me with on the consent screen.");
        ctx.reply_embed(&embed).await
    }
}
