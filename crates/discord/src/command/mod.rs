//! Command framework — both prefix and slash share this surface.
//!
//! A [`Command`] implementation describes its [`CommandMeta`] and exposes an
//! async `execute(ctx)` method. The same trait is used for built-in Rust
//! commands and the [`script::ScriptCommandAdapter`] wrapping Rhai scripts.
//!
//! Slash parameters are declared as [`CommandOption`]s on the meta — they
//! are sent to Discord at registration time so users get autocomplete hints
//! when typing, and the option values are flattened into a single `args`
//! string on dispatch so command implementations stay parser-agnostic.

pub mod builtin;
pub mod context;
pub mod registry;
pub mod script;

pub use context::{CommandContext, InvocationSource};
pub use registry::CommandRegistry;

use std::sync::Arc;

use async_trait::async_trait;
use twilight_model::guild::Permissions;

use tomo_core::error::Result;

/// One parameter declared by a slash command. The same value is also
/// concatenated into `ctx.args()` so prefix-style commands can use it
/// without rewriting.
#[derive(Debug, Clone)]
pub struct CommandOption {
    pub name: String,
    pub description: String,
    pub kind: OptionKind,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    String,
    Integer,
    Boolean,
    User,
    Channel,
}

#[derive(Debug, Clone, Default)]
pub struct CommandMeta {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    /// Category bucket shown in `/help`. Owned so Rhai scripts can supply
    /// arbitrary categories and slot in next to built-ins under the same
    /// heading (`General`, `Utility`, etc.).
    pub category: String,
    pub slash: bool,
    pub prefix: bool,
    pub guild_only: bool,
    pub owner_only: bool,
    /// Discord permissions the invoker must hold in the guild. `None` means
    /// no permission gate (everyone can run). Bot owners *always* bypass
    /// this, on any server.
    pub required_permissions: Option<Permissions>,
    pub options: Vec<CommandOption>,
}

impl CommandMeta {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            category: "General".into(),
            slash: true,
            prefix: true,
            guild_only: false,
            owner_only: false,
            required_permissions: None,
            options: Vec::new(),
        }
    }

    pub fn aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    pub fn category(mut self, c: impl Into<String>) -> Self { self.category = c.into(); self }
    pub fn slash_only(mut self) -> Self { self.slash = true; self.prefix = false; self }
    pub fn prefix_only(mut self) -> Self { self.slash = false; self.prefix = true; self }
    pub fn guild_only(mut self) -> Self { self.guild_only = true; self }
    pub fn owner_only(mut self) -> Self { self.owner_only = true; self }
    /// Require the invoker to hold *every* permission bit in `perm` in the
    /// guild the command runs in. Implies `guild_only` (perm checks need a
    /// guild to evaluate against). Owners bypass this.
    pub fn requires_permissions(mut self, perm: Permissions) -> Self {
        self.required_permissions = Some(perm);
        self.guild_only = true;
        self
    }

    pub fn option(mut self, opt: CommandOption) -> Self {
        self.options.push(opt);
        self
    }

    /// Shortcut: declare a `String` slash option that also feeds `ctx.args()`.
    pub fn string_option(self, name: &str, description: &str, required: bool) -> Self {
        self.option(CommandOption {
            name: name.into(),
            description: description.into(),
            kind: OptionKind::String,
            required,
        })
    }

    pub fn integer_option(self, name: &str, description: &str, required: bool) -> Self {
        self.option(CommandOption {
            name: name.into(),
            description: description.into(),
            kind: OptionKind::Integer,
            required,
        })
    }
}

#[async_trait]
pub trait Command: Send + Sync + 'static {
    fn meta(&self) -> &CommandMeta;
    async fn execute(&self, ctx: CommandContext) -> Result<()>;
}

pub type DynCommand = Arc<dyn Command>;
