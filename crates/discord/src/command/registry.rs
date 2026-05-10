use std::collections::HashMap;
use std::sync::Arc;

use twilight_model::application::command::{Command as DiscordCommand, CommandType};
use twilight_model::id::Id;
use twilight_model::id::marker::GuildMarker;
use twilight_util::builder::command::CommandBuilder;

use tomo_core::error::{Error, Result};

use crate::command::DynCommand;
use crate::state::Bot;

/// Read-only snapshot of every command currently dispatchable.
#[derive(Default)]
pub struct CommandRegistry {
    by_name: HashMap<String, DynCommand>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, cmd: DynCommand) {
        let name = cmd.meta().name.to_ascii_lowercase();
        for alias in &cmd.meta().aliases {
            self.by_name.insert(alias.to_ascii_lowercase(), Arc::clone(&cmd));
        }
        self.by_name.insert(name, cmd);
    }

    pub fn get(&self, name: &str) -> Option<&DynCommand> {
        self.by_name.get(&name.to_ascii_lowercase())
    }

    pub fn iter_unique(&self) -> impl Iterator<Item = &DynCommand> {
        // Walk the map but only yield each underlying command once. We
        // identify uniqueness by data-pointer equality on the Arc — comparing
        // wide pointers directly is ambiguous because they carry vtable
        // metadata.
        let mut seen: Vec<*const ()> = Vec::new();
        let mut out: Vec<&DynCommand> = Vec::new();
        for cmd in self.by_name.values() {
            let ptr = Arc::as_ptr(cmd) as *const ();
            if !seen.contains(&ptr) {
                seen.push(ptr);
                out.push(cmd);
            }
        }
        out.into_iter()
    }

    /// Build the list of Discord application commands for the slash-aware
    /// commands in this registry.
    pub fn to_discord_commands(&self) -> Vec<DiscordCommand> {
        self.iter_unique()
            .filter(|c| c.meta().slash)
            .map(|c| {
                let meta = c.meta();
                CommandBuilder::new(meta.name.clone(), meta.description.clone(), CommandType::ChatInput)
                    .build()
            })
            .collect()
    }
}

/// Register every slash command in the registry with Discord. If
/// `dev_guild` is `Some`, the commands are registered as guild-scoped (fast
/// to update); otherwise they are registered globally (slow rollout but
/// available everywhere).
pub async fn publish_slash(
    bot: &Bot,
    registry: &CommandRegistry,
    dev_guild: Option<Id<GuildMarker>>,
) -> Result<()> {
    let cmds = registry.to_discord_commands();
    let client = bot.http.interaction(bot.identity.application_id);

    match dev_guild {
        Some(guild_id) => {
            client
                .set_guild_commands(guild_id, &cmds)
                .await
                .map_err(|e| Error::config(format!("set_guild_commands: {e}")))?;
        }
        None => {
            client
                .set_global_commands(&cmds)
                .await
                .map_err(|e| Error::config(format!("set_global_commands: {e}")))?;
        }
    }
    Ok(())
}
