//! Bot bootstrap, event loop, and lifecycle.

use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt as _};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use tomo_cache::{PersistentCache, ResourceType};
use twilight_gateway::{Config as GatewayConfig, Event, Shard, StreamExt as _};
use twilight_http::Client;
use twilight_model::application::interaction::{Interaction, InteractionData, InteractionType};
use twilight_model::channel::Message;
use twilight_model::gateway::payload::incoming::{InteractionCreate, MessageCreate};
use twilight_model::id::Id;
use twilight_standby::Standby;

use tomo_core::{Config, Result as CoreResult, Service};
use tomo_db::KvStore;
use tomo_llm::{ConversationStore, LlmRouter, RateLimiter};
use tomo_scripting::ScriptManager;
use tomo_stats::{CommandKind, Stats};

use crate::command::builtin;
use crate::command::context::{CommandContext, InvocationSource};
use crate::command::registry::{publish_slash, CommandRegistry};
use crate::command::script::ScriptCommandAdapter;
use crate::command::{Command, DynCommand};
use crate::llm_mention;
use crate::intents::{event_types, intents};
use crate::state::{Bot, BotIdentity, BotState, LlmContext};
use crate::trigger::{default_builtin as default_triggers, TriggerRegistry};

/// Long-running Discord service.
pub struct DiscordService {
    bot: Bot,
    /// Hot-reload watcher; kept alive for the duration of the service.
    _script_watcher: Option<notify::RecommendedWatcher>,
}

impl DiscordService {
    /// Build the service. The caller (the binary) typically does this once and
    /// hands the result to [`run_all_services`].
    pub async fn bootstrap(
        config: Arc<Config>,
        db: Arc<dyn KvStore>,
    ) -> CoreResult<Self> {
        let http = Arc::new(Client::new(config.discord.token.clone()));

        let identity = BotIdentity::fetch(&http).await?;

        let mut owners = config.discord.owners.clone();
        if owners.is_empty() {
            if let Some(owner) = identity.owner.as_ref() {
                owners.push(owner.id);
            }
        }

        let cache = Arc::new(
            PersistentCache::builder(Arc::clone(&db))
                .resource_types(
                    ResourceType::CHANNEL
                        | ResourceType::GUILD
                        | ResourceType::MEMBER
                        | ResourceType::MESSAGE
                        | ResourceType::ROLE
                        | ResourceType::USER
                        | ResourceType::USER_CURRENT,
                )
                .message_cache_size(200)
                .build()
                .await
                .map_err(|e| tomo_core::Error::config(format!("cache build: {e}")))?,
        );

        let standby = Arc::new(Standby::new());
        let stats = Stats::new(Arc::clone(&db));
        let scripts = Arc::new(ScriptManager::new(&config.script_dir));
        if let Err(e) = scripts.reload().await {
            warn!(error = %e, "initial script load");
        }

        let llm = build_llm(&config).await;
        let requester = tomo_requester::Requester::new()
            .map_err(|e| tomo_core::Error::config(format!("requester: {e}")))?;
        let ocr = build_ocr(&config);

        // Hand the Myon config to the scripting crate's process-wide static
        // *before* the gateway starts dispatching events — otherwise the
        // first batch of MESSAGE_CREATE / VOICE_STATE_UPDATE events would
        // race the init and miss `last_seen` updates.
        init_myon(&config);

        let settings = crate::settings::SettingsStore::new(Arc::clone(&db));

        let bot = Arc::new(BotState {
            http,
            cache,
            standby,
            config: Arc::clone(&config),
            db: Arc::clone(&db),
            stats,
            scripts: Arc::clone(&scripts),
            requester,
            llm,
            settings,
            ocr,
            commands: Arc::new(ArcSwap::from_pointee(CommandRegistry::new())),
            triggers: Arc::new(ArcSwap::from_pointee(TriggerRegistry::new())),
            identity,
            owners,
            started_at: Utc::now(),
            llm_quota_alert_day: std::sync::atomic::AtomicI64::new(0),
        });

        refresh_command_registry(&bot);
        refresh_trigger_registry(&bot);

        if bot.config.discord.enable_slash {
            let registry = bot.commands.load_full();
            if let Err(e) =
                publish_slash(&bot, &registry, bot.config.discord.dev_guild.filter(|_| !bot.config.discord.register_global)).await
            {
                warn!(error = %e, "failed publishing slash commands");
            } else {
                info!("slash commands published");
            }
        }

        // Hot-reload watcher.
        let watcher = if bot.config.enable_hot_reload {
            match tomo_scripting::watcher::spawn(&scripts) {
                Ok(w) => {
                    // Whenever scripts reload we need to rebuild the registries.
                    spawn_registry_refresher(Arc::clone(&bot));
                    Some(w)
                }
                Err(e) => {
                    warn!(error = %e, "hot-reload watcher failed");
                    None
                }
            }
        } else {
            None
        };

        let id = &bot.identity;
        info!(
            bot = %id.full_username(),
            user_id = %id.user_id,
            application_id = %id.application_id,
            application = %id.application_name,
            verified = id.verified,
            public = id.bot_public,
            owners = bot.owners.len(),
            commands = bot.commands.load().iter_unique().count(),
            triggers = bot.triggers.load().iter_builtin().count() + bot.triggers.load().iter_script().count(),
            llm = bot.llm.is_some(),
            "bot ready"
        );
        info!(
            user_id = %id.user_id,
            "tip: LLM mention triggers on `<@{}>` (or a reply to the bot)",
            id.user_id
        );
        if let Some(owner) = id.owner.as_ref() {
            info!(owner_id = %owner.id, owner = %owner.username, "application owner");
        }
        for owner in &bot.owners {
            info!(owner_id = %owner, "authorised for owner_only commands and master prefix");
        }

        Ok(Self { bot, _script_watcher: watcher })
    }

    pub fn bot(&self) -> Bot {
        Arc::clone(&self.bot)
    }
}

#[async_trait]
impl Service for DiscordService {
    fn name(&self) -> &'static str { "discord" }

    async fn run(self: Box<Self>, shutdown: watch::Receiver<bool>) -> CoreResult<()> {
        let bot = self.bot.clone();
        let _watcher = self._script_watcher; // keep alive

        // Reminder delivery loop runs alongside the gateway. Sharing the
        // same shutdown signal means it stops as soon as the bot does.
        tokio::spawn(crate::reminder::run_loop(bot.clone(), shutdown.clone()));

        let token = bot.config.discord.token.clone();
        let gateway_config = GatewayConfig::new(token, intents());

        let shards = match twilight_gateway::create_recommended(
            &bot.http,
            gateway_config,
            |_, builder| builder.build(),
        )
        .await
        {
            Ok(it) => it.collect::<Vec<Shard>>(),
            Err(e) => return Err(tomo_core::Error::config(format!("create_recommended: {e}"))),
        };

        info!(shards = shards.len(), "starting gateway");

        let mut tasks = FuturesUnordered::new();
        for shard in shards {
            tasks.push(tokio::spawn(shard_loop(Arc::clone(&bot), shard, shutdown.clone())));
        }

        while let Some(res) = tasks.next().await {
            if let Err(e) = res {
                error!(error = %e, "shard task panicked");
            }
        }
        Ok(())
    }
}

async fn shard_loop(bot: Bot, mut shard: Shard, mut shutdown: watch::Receiver<bool>) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                info!("shard {:?} shutting down", shard.id());
                break;
            }
            event = shard.next_event(event_types()) => {
                match event {
                    Some(Ok(ev)) => {
                        let bot = Arc::clone(&bot);
                        tokio::spawn(async move { dispatch_event(bot, ev).await });
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "gateway error");
                        // Recoverable errors get logged; twilight reconnects
                        // automatically. We let the loop continue and rely on
                        // the stream returning `None` to exit the shard.
                    }
                    None => break,
                }
            }
        }
    }
}

async fn dispatch_event(bot: Bot, event: Event) {
    // The cache update wipes pre-existing entries for the message id on
    // delete and replaces the entry on update — so we have to snapshot the
    // *previous* state before letting the cache mutate. The snapshot then
    // feeds the edit / delete log handlers, which otherwise have no way to
    // see what the message used to say.
    let pre_dispatch = match &event {
        Event::MessageUpdate(u) => {
            bot.cache.message(u.id).map(|m| (*m).clone())
        }
        Event::MessageDelete(d) => {
            bot.cache.message(d.id).map(|m| (*m).clone())
        }
        _ => None,
    };

    bot.cache.update(&event);
    bot.standby.process(&event);

    match event {
        Event::Ready(ready) => {
            info!(user = %ready.user.name, "gateway ready");
        }
        Event::Resumed => {
            debug!("gateway session resumed");
        }
        Event::MessageCreate(boxed) => on_message(bot, *boxed).await,
        Event::MessageUpdate(boxed) => {
            crate::message_log::handle_update(bot, *boxed, pre_dispatch).await;
        }
        Event::MessageDelete(d) => {
            crate::message_log::handle_delete(bot, d, pre_dispatch).await;
        }
        Event::VoiceStateUpdate(boxed) => {
            // Mirror MessageCreate's hand-off: if the tracked user just
            // joined/left voice, flip `in_voice` and bump `last_seen`.
            // Other users are ignored inside `observe_voice`.
            let now = chrono::Utc::now().timestamp();
            tomo_scripting::myon::observe_voice(
                boxed.0.user_id.get(),
                boxed.0.channel_id.is_some(),
                now,
            );
        }
        Event::InteractionCreate(boxed) => on_interaction(bot, *boxed).await,
        _ => {}
    }
}

async fn on_message(bot: Bot, ev: MessageCreate) {
    let message = ev.0;
    if message.author.bot {
        return;
    }

    bot.stats.record_message(message.author.id, message.guild_id);

    // Feed the Myon tracker. The check (`user_id` match) lives inside
    // `observe_message`; for all other authors this is a single atomic
    // load + compare.
    tomo_scripting::myon::observe_message(
        message.author.id.get(),
        message.timestamp.as_secs(),
    );

    // 1) Prefix commands. Per-guild prefix override (configured via
    // `/settings`) wins over the global default; the master prefix is
    // always available alongside both for owners.
    if bot.config.discord.enable_prefix {
        let guild_prefix = match message.guild_id {
            Some(g) => bot
                .settings
                .get(g)
                .await
                .prefix
                .filter(|s| !s.trim().is_empty()),
            None => None,
        };
        if let Some((name, args)) = parse_prefix(&bot, &message, guild_prefix.as_deref()) {
            if let Some(cmd) = bot.commands.load().get(&name).cloned() {
                run_command(
                    bot.clone(),
                    cmd,
                    InvocationSource::Prefix { msg: Box::new(message.clone()), args },
                )
                .await;
                return;
            }
        }
    }

    // 2) Auto-triggers.
    if bot.config.discord.enable_auto_triggers {
        let registry = bot.triggers.load_full();
        registry.dispatch(&bot, &message).await;
    }

    // 3) LLM mention — handled by the multi-provider router.
    if bot.config.discord.enable_llm
        && bot.llm.is_some()
        && llm_mention::should_respond(&message, &bot)
    {
        tokio::spawn(async move {
            if let Err(e) = llm_mention::handle(bot, message).await {
                warn!(error = %e, "llm mention");
            }
        });
    }
}

async fn on_interaction(bot: Bot, ev: InteractionCreate) {
    let interaction: Interaction = ev.0;
    if !bot.config.discord.enable_slash {
        return;
    }

    match interaction.kind {
        InteractionType::ApplicationCommand => dispatch_application_command(bot, interaction).await,
        InteractionType::ModalSubmit => dispatch_modal_submit(bot, interaction).await,
        // Buttons / select menus are picked up via `Standby` from inside
        // commands that opted in (e.g. the paginator). Anything else just
        // ignored.
        _ => {}
    }
}

async fn dispatch_application_command(bot: Bot, interaction: Interaction) {
    let Some(data) = interaction.data.clone() else { return };
    let InteractionData::ApplicationCommand(cmd_data) = data else { return };

    let name = cmd_data.name.clone();
    let Some(cmd) = bot.commands.load().get(&name).cloned() else {
        warn!(name, "unknown slash command");
        return;
    };

    let args = flatten_options(&cmd_data.options);
    run_command(
        bot,
        cmd,
        InvocationSource::Slash {
            interaction: Box::new(interaction),
            data: Box::new(*cmd_data),
            args,
        },
    )
    .await;
}

/// Modal submits come back with their own `custom_id`. We route by prefix
/// so adding a second modal later is just another match arm.
async fn dispatch_modal_submit(bot: Bot, interaction: Interaction) {
    let Some(data) = interaction.data.clone() else { return };
    let InteractionData::ModalSubmit(modal) = data else { return };

    match modal.custom_id.as_str() {
        crate::command::builtin::settings::MODAL_CUSTOM_ID => {
            crate::command::builtin::settings::handle_submit(bot, interaction, *modal).await;
        }
        other => {
            debug!(custom_id = other, "modal submit with unknown custom_id — ignoring");
        }
    }
}

/// Concatenate the leaf values of a slash command's options into a single
/// whitespace-separated string. Command implementations parse this the same
/// way they parse `args` from a prefix invocation, so option declarations
/// give users hints without forcing a parser rewrite on every command.
fn flatten_options(
    options: &[twilight_model::application::interaction::application_command::CommandDataOption],
) -> String {
    use twilight_model::application::interaction::application_command::CommandOptionValue;
    let mut parts = Vec::new();
    for opt in options {
        match &opt.value {
            CommandOptionValue::String(s) if !s.is_empty() => parts.push(s.clone()),
            CommandOptionValue::Integer(n) => parts.push(n.to_string()),
            CommandOptionValue::Number(n) => parts.push(n.to_string()),
            CommandOptionValue::Boolean(b) => parts.push(b.to_string()),
            CommandOptionValue::User(id) => parts.push(format!("<@{id}>")),
            CommandOptionValue::Channel(id) => parts.push(format!("<#{id}>")),
            CommandOptionValue::Role(id) => parts.push(format!("<@&{id}>")),
            CommandOptionValue::SubCommand(sub) | CommandOptionValue::SubCommandGroup(sub) => {
                parts.push(opt.name.clone());
                parts.push(flatten_options(sub));
            }
            _ => {}
        }
    }
    parts.join(" ")
}

async fn run_command(bot: Bot, cmd: DynCommand, source: InvocationSource) {
    let meta = cmd.meta().clone();

    let author = match &source {
        InvocationSource::Prefix { msg, .. } => msg.author.id,
        InvocationSource::Slash { interaction, .. } => {
            interaction.author_id().unwrap_or(Id::new(1))
        }
    };
    let guild_id = match &source {
        InvocationSource::Prefix { msg, .. } => msg.guild_id,
        InvocationSource::Slash { interaction, .. } => interaction.guild_id,
    };

    // Permission gates. Bot owners bypass everything (owner_only, guild_only,
    // required_permissions) on any server — useful for emergency admin and
    // for the bot author testing in their own dev guild without role
    // wrangling.
    let is_owner = bot.is_owner(author);
    if !is_owner {
        if meta.owner_only {
            warn!(name = %meta.name, user = %author, "non-owner attempted owner command");
            return;
        }
        if meta.guild_only && guild_id.is_none() {
            warn!(name = %meta.name, "guild-only command used in DM");
            return;
        }
        if let Some(required) = meta.required_permissions {
            let Some(gid) = guild_id else {
                warn!(name = %meta.name, "permission-gated command used in DM");
                return;
            };
            match crate::permissions::resolve(&bot, author, gid).await {
                Ok(perms) if perms.contains(required) => {}
                Ok(_) => {
                    warn!(
                        name = %meta.name,
                        user = %author,
                        guild = %gid,
                        required = ?required,
                        "user lacks required permissions"
                    );
                    return;
                }
                Err(e) => {
                    warn!(
                        name = %meta.name,
                        user = %author,
                        guild = %gid,
                        error = %e,
                        "permission calc failed even after hydrate — denying"
                    );
                    return;
                }
            }
        }
    }

    let ctx = CommandContext::new(Arc::clone(&bot), source);
    let kind = if ctx.is_slash() { CommandKind::Slash } else { CommandKind::Prefix };
    let start = Instant::now();
    let result = cmd.execute(ctx).await;
    bot.stats.record_command(
        &meta.name,
        kind,
        author,
        guild_id,
        result.is_ok(),
        start.elapsed(),
    );
    if let Err(e) = result {
        warn!(name = %meta.name, error = %e, "command failed");
    }
}

/// Strip the prefix from `msg.content` and split off the command name +
/// args. `guild_prefix` is the per-guild override from settings; when
/// `None` we fall back to the global `TOMO_PREFIX`. The master prefix is
/// always matched first for owners regardless of any override.
fn parse_prefix(
    bot: &Bot,
    msg: &Message,
    guild_prefix: Option<&str>,
) -> Option<(String, String)> {
    let content = msg.content.trim_start();
    let server_prefix = guild_prefix.unwrap_or(bot.config.discord.prefix.as_str());
    let prefix = if bot.is_owner(msg.author.id) {
        try_prefix(content, &bot.config.discord.master_prefix)
            .or_else(|| try_prefix(content, server_prefix))?
    } else {
        try_prefix(content, server_prefix)?
    };
    let rest = content[prefix.len()..].trim_start();
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim_start()),
        None => (rest, ""),
    };
    if name.is_empty() {
        return None;
    }
    Some((name.to_ascii_lowercase(), args.to_string()))
}

fn try_prefix<'a>(content: &'a str, prefix: &'a str) -> Option<&'a str> {
    // ASCII-case-insensitive — typing `Tomo>help`, `TOMO>help`, or
    // `tomo>HELP` all dispatch the same. Prefixes themselves are typically
    // ASCII; for non-ASCII prefixes the comparison falls back to exact
    // string-equality on the leading slice (still safe, just stricter).
    if prefix.is_empty() {
        return None;
    }
    let head = content.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        Some(prefix)
    } else {
        None
    }
}

// ---------- Registry refresh ----------

/// Rebuild the command registry from built-in commands + the current script
/// registry snapshot. Atomically swaps the result in.
pub fn refresh_command_registry(bot: &Bot) {
    let mut registry = CommandRegistry::new();
    for cmd in builtin::all() {
        registry.insert(cmd);
    }
    let script_snapshot = bot.scripts.registry();
    let mut inserted: std::collections::HashSet<String> = std::collections::HashSet::new();
    for cmd in script_snapshot.commands.values() {
        if !inserted.insert(cmd.name.clone()) {
            // Aliases share the underlying command — register once.
            continue;
        }
        let adapter: Arc<dyn Command> =
            Arc::new(ScriptCommandAdapter::new(cmd.clone(), Arc::clone(&bot.scripts)));
        registry.insert(adapter);
    }
    bot.commands.store(Arc::new(registry));
}

pub fn refresh_trigger_registry(bot: &Bot) {
    let script_snapshot = bot.scripts.registry();
    let registry = TriggerRegistry::new()
        .with_builtin(default_triggers())
        .with_scripts(Arc::clone(&bot.scripts), script_snapshot.triggers.clone());
    bot.triggers.store(Arc::new(registry));
}

fn spawn_registry_refresher(bot: Bot) {
    // Poll the script registry every 500ms. A real implementation could hook
    // into the watcher notify channel directly; polling is simpler and the
    // cost is negligible.
    tokio::spawn(async move {
        let mut last_ptr = Arc::as_ptr(&bot.scripts.registry()) as usize;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let now = Arc::as_ptr(&bot.scripts.registry()) as usize;
            if now != last_ptr {
                last_ptr = now;
                refresh_command_registry(&bot);
                refresh_trigger_registry(&bot);
                debug!("registries refreshed from hot-reload");
            }
        }
    });
}

fn build_ocr(config: &Config) -> Vec<crate::state::OcrSlot> {
    use crate::state::OcrSlot;
    use tomo_core::config::OcrEngineFiles;

    let Some(cfg) = config.ocr.as_ref() else { return Vec::new() };

    fn try_load(name: &'static str, files: Option<&OcrEngineFiles>) -> Option<OcrSlot> {
        let files = files?;
        for path in [&files.det, &files.rec, &files.keys] {
            if !path.exists() {
                warn!(engine = name, missing = %path.display(), "OCR file missing; skipping engine");
                return None;
            }
        }
        match ocr_rs::OcrEngine::new(&files.det, &files.rec, &files.keys, None) {
            Ok(engine) => {
                info!(engine = name, det = %files.det.display(), "OCR engine ready");
                Some(OcrSlot { name, engine: Arc::new(engine) })
            }
            Err(e) => {
                warn!(engine = name, error = ?e, "OCR engine failed to load");
                None
            }
        }
    }

    let mut out = Vec::new();
    out.extend(try_load("latin", cfg.latin.as_ref()));
    out.extend(try_load("cjk", cfg.cjk.as_ref()));
    out
}

/// Hand the Myon config from env down to the scripting crate's
/// process-wide static. Skipped when `MYON_USER_ID` isn't set — the
/// tracker stays dormant and the time script's myon row hides itself.
fn init_myon(config: &Config) {
    let Some(cfg) = config.myon.as_ref() else { return };
    let tz: chrono_tz::Tz = match cfg.timezone.parse() {
        Ok(t) => t,
        Err(e) => {
            warn!(
                tz = %cfg.timezone,
                error = %e,
                "myon: invalid timezone string — tracker disabled"
            );
            return;
        }
    };
    info!(
        user = cfg.user_id,
        tz = %cfg.timezone,
        sleep_hour = cfg.sleep_hour,
        idle_minutes = cfg.idle_seconds / 60,
        "myon: tracker enabled"
    );
    tomo_scripting::myon::init(tomo_scripting::myon::MyonConfig {
        user_id: cfg.user_id,
        timezone: tz,
        label: cfg.label.clone(),
        sleep_hour: cfg.sleep_hour,
        idle_seconds: cfg.idle_seconds,
    });
}

/// Build the multi-provider router from the parsed config. Returns `None`
/// when LLM responses should stay off (no key for any provider, or
/// `TOMO_ENABLE_LLM=false`).
async fn build_llm(config: &Config) -> Option<LlmContext> {
    let Some(cfg) = config.llm.clone() else {
        info!("llm: disabled (see tomo-core config logs above for the reason)");
        return None;
    };

    let context_messages = cfg.context_messages;
    let rate_per_min = cfg.rate_limit_per_minute;

    let router = match LlmRouter::from_config(cfg) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = ?e, "llm: router build failed — disabled");
            return None;
        }
    };

    if router.is_empty() {
        warn!("llm: resolved chain is empty after filtering — disabled");
        return None;
    }

    let chain_summary: Vec<String> = router
        .entries()
        .iter()
        .map(|e| format!("{}/{}", e.provider.name(), e.model))
        .collect();
    info!(chain = ?chain_summary, "llm: fallback chain configured");

    let router = Arc::new(router);
    let conversations = Arc::new(ConversationStore::new(context_messages.max(2)));
    let rate_limit = Arc::new(RateLimiter::per_minute(rate_per_min));

    Some(LlmContext {
        router,
        conversations,
        rate_limit,
    })
}

#[cfg(test)]
mod tests {
    use super::try_prefix;

    #[test]
    fn matches_exact_prefix() {
        assert_eq!(try_prefix("tomo>help", "tomo>"), Some("tomo>"));
    }

    #[test]
    fn matches_case_insensitive_prefix() {
        assert_eq!(try_prefix("Tomo>help", "tomo>"), Some("tomo>"));
        assert_eq!(try_prefix("TOMO>HELP", "tomo>"), Some("tomo>"));
        assert_eq!(try_prefix("tOmO>help", "tomo>"), Some("tomo>"));
    }

    #[test]
    fn rejects_wrong_prefix() {
        assert_eq!(try_prefix("foo>help", "tomo>"), None);
    }

    #[test]
    fn rejects_short_content() {
        assert_eq!(try_prefix("tom", "tomo>"), None);
    }

    #[test]
    fn rejects_empty_prefix() {
        assert_eq!(try_prefix("anything", ""), None);
    }
}

