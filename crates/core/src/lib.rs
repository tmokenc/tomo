//! Cross-cutting types shared by every crate in the workspace.
//!
//! Anything that the discord crate, scripting crate, gemini crate, etc. need
//! to agree on (config, error types, identity helpers, the [`Service`] trait
//! that lets the binary spin up multiple long-running services side by side)
//! lives here.

pub mod config;
pub mod error;
pub mod service;

pub use config::Config;
pub use error::{Error, Result};
pub use service::Service;
