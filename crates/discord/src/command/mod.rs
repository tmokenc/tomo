//! Command framework — both prefix and slash share this surface.
//!
//! A [`Command`] implementation describes its [`CommandMeta`] and exposes an
//! async `execute(ctx)` method. The same trait is used for built-in Rust
//! commands and the [`script::ScriptCommandAdapter`] wrapping Rhai scripts.

pub mod builtin;
pub mod context;
pub mod registry;
pub mod script;

pub use context::{CommandContext, InvocationSource};
pub use registry::CommandRegistry;

use std::sync::Arc;

use async_trait::async_trait;

use tomo_core::error::Result;

/// Group label used in the help embed.
#[derive(Debug, Clone, Default)]
pub struct CommandMeta {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub category: &'static str,
    pub slash: bool,
    pub prefix: bool,
    pub guild_only: bool,
    pub owner_only: bool,
}

impl CommandMeta {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            category: "General",
            slash: true,
            prefix: true,
            guild_only: false,
            owner_only: false,
        }
    }

    pub fn aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    pub fn category(mut self, c: &'static str) -> Self { self.category = c; self }
    pub fn slash_only(mut self) -> Self { self.slash = true; self.prefix = false; self }
    pub fn prefix_only(mut self) -> Self { self.slash = false; self.prefix = true; self }
    pub fn guild_only(mut self) -> Self { self.guild_only = true; self }
    pub fn owner_only(mut self) -> Self { self.owner_only = true; self }
}

#[async_trait]
pub trait Command: Send + Sync + 'static {
    fn meta(&self) -> &CommandMeta;
    async fn execute(&self, ctx: CommandContext) -> Result<()>;
}

pub type DynCommand = Arc<dyn Command>;
