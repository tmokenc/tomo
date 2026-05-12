//! Discord bot service for Tomo.
//!
//! Wires twilight (gateway + http + cache + standby) up against the shared
//! command/trigger framework and exposes a [`DiscordService`] that the binary
//! launches alongside any other services (telegram, web admin, etc.).

pub mod anilist_view;
pub mod command;
pub mod ehentai_view;
pub mod llm_mention;
pub mod intents;
pub mod nhentai_view;
pub mod prelude;
pub mod reminder;
pub mod rpc_server;
pub mod runtime;
pub mod state;
pub mod trigger;
pub mod util;
pub mod vndb_view;

pub use command::{Command, CommandContext, CommandMeta, CommandRegistry, InvocationSource};
pub use rpc_server::{RpcConfig, RpcService};
pub use runtime::DiscordService;
pub use state::{Bot, BotState};
pub use trigger::{Trigger, TriggerContext, TriggerRegistry};
