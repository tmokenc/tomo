use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use rhai::AST;

use crate::trigger::TriggerMatcher;

#[derive(Clone)]
pub struct ScriptCommand {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    /// Help-embed category override. When unset the adapter falls back to
    /// the default category (currently "General"), so scripts blend into the
    /// help listing alongside built-in commands.
    pub category: Option<String>,
    pub slash: bool,
    pub prefix: bool,
    pub guild_only: bool,
    pub source_path: PathBuf,
    pub ast: Arc<AST>,
}

#[derive(Clone)]
pub struct ScriptTrigger {
    pub name: String,
    pub matcher: TriggerMatcher,
    pub source_path: PathBuf,
    pub ast: Arc<AST>,
}

/// Snapshot of all currently loaded scripts. Replaced atomically on hot-reload
/// via [`crate::manager::ScriptManager`].
#[derive(Default, Clone)]
pub struct ScriptRegistry {
    pub commands: HashMap<String, ScriptCommand>,
    pub triggers: Vec<ScriptTrigger>,
}

impl ScriptRegistry {
    pub fn lookup_command(&self, name: &str) -> Option<&ScriptCommand> {
        if let Some(cmd) = self.commands.get(name) {
            return Some(cmd);
        }
        // Fall back to alias lookup. Aliases live as separate keys, so this
        // path only triggers when callers used the canonical name and missed.
        self.commands
            .values()
            .find(|c| c.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
    }
}
