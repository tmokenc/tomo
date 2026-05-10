//! Auto-trigger framework.
//!
//! An auto-trigger fires on every non-bot message that satisfies its
//! [`MatchSpec`]. Triggers can be defined two ways:
//! 1. **Rust** — implement [`Trigger`] directly. Useful when the action needs
//!    rich access to internal state.
//! 2. **Rhai** — drop a script under `<script_dir>/triggers/`. Its `meta()`
//!    must include a `match` map; the loader translates that into a built-in
//!    matcher.
//!
//! The current registry snapshot is held in an [`ArcSwap`] so it can be
//! swapped atomically on hot-reload.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use tokio::time;
use tracing::{debug, warn};
use twilight_model::channel::Message;

use tomo_core::error::{Error, Result};
use tomo_scripting::{ScriptCtx, ScriptManager, ScriptTrigger};

use crate::command::script::apply_actions;
use crate::command::{CommandContext, InvocationSource};
use crate::state::Bot;

#[async_trait]
pub trait Trigger: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn matches(&self, msg: &Message, bot: &Bot) -> bool;
    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()>;
}

pub struct TriggerContext<'a> {
    pub bot: Bot,
    pub message: &'a Message,
}

#[derive(Default)]
pub struct TriggerRegistry {
    builtin: Vec<Arc<dyn Trigger>>,
    /// Triggers built from Rhai scripts. Reload-aware.
    scripts: Vec<ScriptTriggerEntry>,
}

struct ScriptTriggerEntry {
    inner: ScriptTrigger,
    manager: Arc<ScriptManager>,
}

impl TriggerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin(mut self, triggers: Vec<Arc<dyn Trigger>>) -> Self {
        self.builtin = triggers;
        self
    }

    pub fn with_scripts(mut self, manager: Arc<ScriptManager>, triggers: Vec<ScriptTrigger>) -> Self {
        self.scripts = triggers
            .into_iter()
            .map(|inner| ScriptTriggerEntry { inner, manager: Arc::clone(&manager) })
            .collect();
        self
    }

    pub fn iter_builtin(&self) -> impl Iterator<Item = &Arc<dyn Trigger>> {
        self.builtin.iter()
    }

    pub fn iter_script(&self) -> impl Iterator<Item = &ScriptTrigger> {
        self.scripts.iter().map(|e| &e.inner)
    }

    /// Walk every trigger, run anything that matches.
    pub async fn dispatch(&self, bot: &Bot, message: &Message) {
        for trigger in &self.builtin {
            if trigger.matches(message, bot) {
                debug!(name = trigger.name(), "builtin trigger matched");
                let ctx = TriggerContext { bot: Arc::clone(bot), message };
                if let Err(e) = trigger.execute(ctx).await {
                    warn!(name = trigger.name(), error = %e, "trigger failed");
                }
            }
        }

        // Script triggers need a probe — gather it once.
        let probe = build_probe(message, bot);
        for entry in &self.scripts {
            if entry.inner.matcher.matches(&probe) {
                debug!(name = %entry.inner.name, "script trigger matched");
                if let Err(e) = run_script_trigger(bot, &entry.inner, &entry.manager, message).await {
                    warn!(name = %entry.inner.name, error = %e, "script trigger failed");
                }
            }
        }
    }
}

fn build_probe<'a>(message: &'a Message, bot: &Bot) -> tomo_scripting::trigger::MessageProbe<'a> {
    use tomo_scripting::trigger::MessageProbe;
    let has_image = message
        .attachments
        .iter()
        .any(|a| a.content_type.as_deref().map(|t| t.starts_with("image/")).unwrap_or(false));
    let has_attachment = !message.attachments.is_empty();
    let mentions_bot = message.mentions.iter().any(|m| m.id == bot.identity.user_id);
    MessageProbe {
        content: &message.content,
        has_image,
        has_attachment,
        mentions_bot,
    }
}

async fn run_script_trigger(
    bot: &Bot,
    trigger: &ScriptTrigger,
    manager: &Arc<ScriptManager>,
    message: &Message,
) -> Result<()> {
    let (ctx, rx) = ScriptCtx::channel(
        message.channel_id.get(),
        message.guild_id.map(|g| g.get()),
        message.author.id.get(),
        message.id.get(),
        message.content.clone(),
        bot.identity.username.clone(),
        bot.started_at.timestamp(),
        chrono::Utc::now().timestamp(),
    );
    let actions = time::timeout(
        Duration::from_secs(3),
        manager.run_trigger(trigger, ctx, rx),
    )
    .await
    .map_err(|_| Error::script("trigger timed out"))??;

    // Borrow the same machinery commands use to apply actions.
    let cmd_ctx = CommandContext::new(
        Arc::clone(bot),
        InvocationSource::Prefix {
            msg: Box::new(message.clone()),
            args: String::new(),
        },
    );
    apply_actions(&cmd_ctx, actions).await
}

// ---------- Built-in triggers (examples) ----------

/// Quick implementation factory: regex-matched response.
pub struct RegexResponderTrigger {
    name: &'static str,
    regex: Regex,
    response: String,
}

impl RegexResponderTrigger {
    pub fn new(name: &'static str, pattern: &str, response: impl Into<String>) -> Result<Self> {
        Ok(Self {
            name,
            regex: Regex::new(pattern)
                .map_err(|e| Error::config(format!("trigger regex {name}: {e}")))?,
            response: response.into(),
        })
    }
}

#[async_trait]
impl Trigger for RegexResponderTrigger {
    fn name(&self) -> &str { self.name }
    fn matches(&self, msg: &Message, _bot: &Bot) -> bool { self.regex.is_match(&msg.content) }
    async fn execute(&self, ctx: TriggerContext<'_>) -> Result<()> {
        ctx.bot
            .http
            .create_message(ctx.message.channel_id)
            .content(&self.response)
            .reply(ctx.message.id)
            .await
            .map_err(|e| Error::config(format!("trigger send: {e}")))?;
        Ok(())
    }
}

/// Default set the binary wires up unless overridden.
pub fn default_builtin() -> Vec<Arc<dyn Trigger>> {
    let mut v: Vec<Arc<dyn Trigger>> = Vec::new();
    if let Ok(t) = RegexResponderTrigger::new("greet", r"(?i)^hello\s+tomo\b", "Hi there! 👋") {
        v.push(Arc::new(t));
    }
    v
}

// Re-export TriggerMatcher path for downstream crates.
pub use tomo_scripting::TriggerMatcher as ScriptTriggerMatcher;
