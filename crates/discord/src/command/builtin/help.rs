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
            groups.entry(cmd.meta().category).or_default().push(cmd.meta());
        }

        let mut embed = Embed2::info().title("Tomo — Help").description(format!(
            "Prefix `{}` or use slash commands. {} commands available.",
            ctx.bot.config.discord.prefix,
            snapshot.iter_unique().count()
        ));

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
            embed = embed.field(category.to_string(), body, false);
        }
        ctx.reply_embed(&embed).await
    }
}
