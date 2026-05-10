# Tomo

A modular, multi-service Discord bot written in Rust. Inspired by
[`tomoka-rs`](https://gitlab.com/tmokenc/tomoka-rs), redesigned around the
[twilight](https://twilight.rs) ecosystem and [Rhai](https://rhai.rs) for
hot-reloadable scripts.

## Highlights

- **Dual command surface** — every command works as both a slash command and a
  prefix command, configurable via `.env`.
- **Rhai scripting** — drop a `.rhai` file under `scripts/commands/` or
  `scripts/triggers/` and it loads automatically. Edits hot-reload without a
  restart.
- **Rust escape hatch** — complex commands that need direct access to bot
  internals (stats, paginator, etc.) live as Rust impls of the same `Command`
  trait that script commands use.
- **Auto-triggers** — fire on patterns (regex, has-image, contains-keyword,
  starts-with, mentions-bot, …). Definable in Rhai or Rust.
- **Embed framework** — `Embed2` builder with theme colours
  (info/success/error/warning/lovely), helpers for author/footer/timestamp.
- **Pagination** — generic `PageSource` trait with a button-driven paginator
  on top.
- **Gemini integration** — when a non-bot user `@`-mentions Tomo, the bot
  asks Gemini and streams the answer back. Per-user rate-limited, per-channel
  short-term memory.
- **Statistics** — every command, message, and Gemini call is counted with an
  embedded LSM-tree DB (fjall, swappable via the `tomo_db::KvStore` trait).
- **Service framework** — the binary launches a `Vec<Box<dyn Service>>`.
  Today that is just `DiscordService`; tomorrow it can include
  `TelegramService`, an admin HTTP server, etc., sharing the same db and
  config.

## Layout

```
tomo/
├── Cargo.toml             workspace root
├── .env.example           copy to `.env` and fill in
├── scripts/
│   ├── commands/*.rhai    user-invocable commands
│   └── triggers/*.rhai    auto-triggers
├── frontend/              Yew SPA (independent crate; built with trunk)
│   ├── Cargo.toml
│   ├── index.html
│   ├── style.css
│   └── src/
└── crates/
    ├── core/              shared types, config loader, Service trait
    ├── db/                async KvStore trait + fjall backend (bytes::Bytes)
    ├── embed/             Embed2 builder + Embedable trait
    ├── pagination/        Paginator + PageSource
    ├── stats/             event counters via KvStore
    ├── gemini/            Gemini REST client + rate-limit + history
    ├── scripting/         Rhai engine, hot-reload, registry
    ├── rpc/               tonic proto + generated client/server + auth
    ├── discord/           twilight bot service, dispatch, RPC server impl
    ├── admin/             axum web service: OAuth, REST API, serves Yew SPA
    └── tomo/              binary entry point — launches every enabled service
```

## Services

| Service               | Purpose                                                       |
| --------------------- | ------------------------------------------------------------- |
| `DiscordService`      | Gateway, commands, triggers, gemini mention handler           |
| `RpcService`          | gRPC control plane backed by `BotState`                       |
| `AdminService`        | Web UI (Yew) + OAuth + REST API; gRPC client of the bot       |

`crates/tomo/src/main.rs` reads env toggles (`TOMO_ENABLE_RPC`, `TOMO_ENABLE_ADMIN`) and only spins up what's enabled, so a minimal deployment is just `DiscordService`.

## Inter-service communication

Services talk to each other through **gRPC (tonic)** — see `crates/rpc/proto/tomo.proto`. Today both servers live in the same process, but the gRPC boundary means future services (Telegram bot, batch scrapers, …) can run on a different host and reach the bot identically.

Security:

* **Discord bot:** owner-only commands gated by `BotState::is_owner`; master prefix only triggered for owners.
* **gRPC server:** binds to `127.0.0.1` by default; supports a Bearer token via `TOMO_RPC_TOKEN`. Privileged endpoints (`ReloadScripts`) additionally re-check `is_owner` against the requester id.
* **Admin service:** binds to `127.0.0.1`; signed `HttpOnly` `SameSite=Lax` session cookies; OAuth with state + PKCE; ownership checked against the bot via gRPC (never trusts the client); CSP, X-Frame-Options=DENY, HSTS, Referrer-Policy on every response.
* **Yew frontend:** session cookie is `HttpOnly` so JS cannot read it; fetch uses `credentials: same-origin`; 401s redirect to `/login`.

## Quick start

```sh
git clone https://github.com/tmokenc/tomo
cd tomo
cp .env.example .env
# put your Discord token in DISCORD_TOKEN
# (optional) put a Gemini key in GEMINI_API_KEY

# 1. Just the bot:
cargo run --release

# 2. With the admin web UI:
#   - set TOMO_ENABLE_ADMIN=true in .env
#   - fill DISCORD_OAUTH_* and TOMO_ADMIN_SESSION_SECRET
#   - build the frontend (requires `cargo install trunk`):
cd frontend && trunk build --release && cd ..
#   - run the binary; the admin UI lives at http://127.0.0.1:8080
cargo run --release
```

The bot creates `./data/` (database) and watches `./scripts/` on first run. The admin SPA is served from `./frontend/dist/`.

### Frontend development

```sh
cd frontend
trunk serve
```

This starts a dev server on `127.0.0.1:8081` with a proxy to the admin backend at `127.0.0.1:8080`. Edits to Yew components hot-reload in the browser.

### Required env vars

| Variable           | Default         | Notes                                           |
| ------------------ | --------------- | ----------------------------------------------- |
| `DISCORD_TOKEN`    | _required_      | Bot token                                       |
| `TOMO_PREFIX`      | `tomo>`         | Prefix for prefix-style commands                |
| `TOMO_MASTER_PREFIX` | `%`           | Owner-only prefix                               |
| `TOMO_OWNERS`      | _app owner_     | Comma-separated user IDs                        |
| `TOMO_DATA_DIR`    | `./data`        | fjall database directory                        |
| `TOMO_SCRIPT_DIR`  | `./scripts`     | Where to look for Rhai scripts                  |
| `GEMINI_API_KEY`   | _disables Gemini if unset_                                          |
| `GEMINI_MODEL`     | `gemini-2.5-flash` |                                              |

See `.env.example` for the full list including toggles for prefix/slash/Gemini/auto-triggers/hot-reload.

## Built-in commands

| Command   | What it does                                       |
| --------- | -------------------------------------------------- |
| `ping`    | Pong + latency                                     |
| `info`    | Bot info, uptime, owners                           |
| `stats`   | Global, per-user, per-server counters              |
| `top`     | Top 10 commands by use                             |
| `help`    | Auto-generated help embed grouped by category      |
| `reload`  | Reload scripts on disk (owner only)                |

Anything beyond these lives in `scripts/`.

## Adding a Rhai command

```rhai
// scripts/commands/dice.rhai
fn meta() {
    #{
        name: "dice",
        description: "Roll a six-sided die.",
        aliases: ["d6"],
    }
}

fn execute(ctx) {
    let r = random_int(1, 7);
    ctx.embed("🎲 Roll", `You rolled a **${r}**.`, color_info());
}
```

Save the file and the next message will see the new command — no restart.

## Adding an auto-trigger

```rhai
// scripts/triggers/wave.rhai
fn meta() {
    #{
        name: "wave",
        match: #{ regex: "(?i)\\b(hi|hello|hey)\\b" },
    }
}

fn execute(ctx) {
    ctx.react("👋");
}
```

Match shapes accepted: `regex`, `has_image`, `has_attachment`, `contains`,
`starts_with`, `mentions_bot`. See `scripts/README.md`.

## Owners

Tomo treats anyone listed in `TOMO_OWNERS` (or the application owner if that
variable is empty) as a bot owner. Owners can:

- Use the master prefix (`TOMO_MASTER_PREFIX`, default `%`) in addition to the
  normal prefix.
- Run `owner_only` commands like `reload`.

Add a Rust command flagged with `.owner_only()` in its `CommandMeta`, or check
`bot.is_owner(user_id)` from anywhere.

## Adding a Rust command

```rust
use std::sync::LazyLock;
use async_trait::async_trait;
use tomo_discord::prelude::*;

pub struct UptimeCommand;

#[async_trait]
impl Command for UptimeCommand {
    fn meta(&self) -> &CommandMeta {
        static META: LazyLock<CommandMeta> = LazyLock::new(|| {
            CommandMeta::new("uptime", "How long the bot has been up.")
        });
        &META
    }
    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let up = ctx.bot.uptime();
        ctx.reply(&format!("Up for `{up}`")).await
    }
}
```

Register it in `crates/discord/src/command/builtin.rs::all()`.

## Adding another service later

A new service (Telegram bot, web admin, etc.) just needs to implement
`tomo_core::Service` and get pushed onto the `services` vec in
`crates/tomo/src/main.rs`. Shared state (`db`, `config`) is already cloneable.

## Swapping the database

The discord crate (and stats, and any future crate) talks to `Arc<dyn KvStore>`.
To swap fjall for, say, SurrealDB or PostgreSQL:

1. Add `crates/db/src/your_backend.rs` implementing `KvStore`.
2. Pick it in `crates/tomo/src/main.rs`.

## License

MIT.
