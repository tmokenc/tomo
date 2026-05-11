use std::collections::BTreeMap;
use std::sync::LazyLock;

use async_trait::async_trait;

use tomo_core::error::Result;
use tomo_embed::Embed2;

use crate::command::{Command, CommandContext, CommandMeta};

pub struct HelpCommand;

#[async_trait]
impl Command for HelpCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("help", "Show available commands")
                .aliases(["h", "commands"])
                .category("General")
        });
        &META
    }

    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let snapshot = ctx.bot.commands.load();
        let mut groups: BTreeMap<&str, Vec<&CommandMeta>> = BTreeMap::new();
        for cmd in snapshot.iter_unique() {
            groups
                .entry(cmd.meta().category.as_str())
                .or_default()
                .push(cmd.meta());
        }
        let unique = snapshot.iter_unique().count();

        let id = &ctx.bot.identity;
        let mut embed = Embed2::info()
            .title(format!("{} — Help", id.username))
            .description(format!(
                "Prefix `{}` or use slash commands. **{unique}** commands across **{}** categories.",
                ctx.bot.config.discord.prefix,
                groups.len(),
            ))
            .author_with(id.full_username(), id.avatar_url.clone(), None::<String>)
            .footer(format!(
                "Tip: `{}<command>` runs a prefix command — slash works too.",
                ctx.bot.config.discord.prefix,
            ))
            .timestamp_now();

        if let Some(url) = id.avatar_url.as_ref() {
            embed = embed.thumbnail(url.clone());
        }

        for (category, mut metas) in groups {
            metas.sort_by(|a, b| a.name.cmp(&b.name));
            let body = metas
                .iter()
                .map(|m| {
                    let badges = match (m.prefix, m.slash) {
                        (true, true) => "[`p`/`s`]",
                        (true, false) => "[`p`]",
                        (false, true) => "[`s`]",
                        _ => "[ ]",
                    };
                    format!("`{}` {} — {}", m.name, badges, m.description)
                })
                .collect::<Vec<_>>()
                .join("\n");
            embed = embed.field(format!("{category} ({})", metas.len()), body, false);
        }
        ctx.reply_embed(&embed).await
    }
}
