//! Rhai-based scripting layer.
//!
//! Scripts live under `<script_dir>/commands/*.rhai` and
//! `<script_dir>/triggers/*.rhai`. Each script defines two functions:
//!
//! ```text
//! fn meta()             // returns a #{ ... } describing the command/trigger
//! fn execute(ctx)       // runs when the command is invoked / trigger matches
//! ```
//!
//! Scripts cannot perform IO directly. Instead `ctx` accumulates a list of
//! [`ScriptAction`]s (`reply`, `react`, `send`, ...). When the script returns,
//! the host (`tomo-discord`) drains the buffer and applies the side effects.
//! That keeps the surface small, async-free, and easy to unit-test.

pub mod ctx;
pub mod embed;
pub mod engine;
pub mod loader;
pub mod manager;
pub mod registry;
pub mod trigger;
pub mod watcher;

pub use ctx::{ScriptAction, ScriptCtx, ScriptInit};
pub use embed::ScriptEmbed;
pub use manager::ScriptManager;
pub use registry::{ScriptCommand, ScriptRegistry, ScriptTrigger};
pub use trigger::TriggerMatcher;
