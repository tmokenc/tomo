//! Common imports for command implementations.

pub use async_trait::async_trait;
pub use std::sync::Arc;

pub use twilight_model::channel::Message;
pub use twilight_model::gateway::payload::incoming::InteractionCreate;
pub use twilight_model::id::Id;
pub use twilight_model::id::marker::{
    ApplicationMarker, ChannelMarker, GuildMarker, MessageMarker, UserMarker,
};

pub use tomo_core::error::{Error, Result};
pub use tomo_embed::{Color, Embed2, Embedable};

pub use crate::command::{Command, CommandContext, CommandMeta, InvocationSource};
pub use crate::state::{Bot, BotState};
