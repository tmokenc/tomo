# Tomo

A modular, multi-service Discord bot written in Rust. Inspired by
[`tomoka-rs`](https://gitlab.com/tmokenc/tomoka-rs), redesigned around the
[twilight](https://twilight.rs) ecosystem and [Rhai](https://rhai.rs) for
hot-reloadable scripts.

## Highlights

- **Dual command surface** — every command works as both a slash command and a
  prefix command. Prefix matching is ASCII-case-insensitive
  (`Tomo>help`, `TOMO>help`, `tomo>HELP` all work).
- **Rhai scripting** — drop a `.rhai` file under `scripts/commands/` or
  `scripts/triggers/` and it loads automatically. Edits hot-reload without a
  restart, and scripts can declare their own help category so they slot in
  next to built-ins.
- **Rust escape hatch** — complex commands that need direct access to bot
  internals (stats, paginator, OCR engines, requester, …) live as Rust impls
  of the same `Command` trait that script commands use.
- **Auto-triggers** — fire on patterns. Built-ins do gallery / VN lookups
  on bare ids; script triggers can match `regex`, `has_image`,
  `has_attachment`, `contains`, `starts_with`, or `mentions_bot`.
- **Embed framework** — `Embed2` builder with theme colours
  (info/success/error/warning/lovely) plus helpers for author, footer,
  timestamp, image, thumbnail, and per-attachment images. Empty
  values/titles/fields are silently dropped so the bot never ships a
  payload Discord will 400.
- **Pagination** — generic `PageSource` trait with a button-driven
  paginator. Multi-result lookups (AniList, VNDB) hydrate each entry
  **on demand** when the user lands on its page, caching as they go —
  no upfront fetch tax.
- **Persistent cache** — `tomo-cache` is a drop-in replacement for
  `twilight-cache-inmemory` backed by the same `tomo-db` KvStore. Hot
  state lives in DashMaps for sync access; every mutation ships to a
  background writer that batches into fjall. Survives restarts.
- **Gemini integration** — when a non-bot user `@`-mentions Tomo (or
  replies to its messages), the bot asks Gemini and posts the answer.
  Features: model fallback chain with per-model 429 cooldowns, per-request
  context (server/channel/topic, replied-message excerpt, OCR of nearby
  images), per-user rate-limit, per-channel short-term memory, once-per-day
  owner DM when every model in the chain is exhausted.
- **OCR (PaddleOCR via MNN)** — `tomo>ocr` extracts text from attached
  images. Optionally configure both the Latin and CJK engines to cover
  English / Czech / Vietnamese / Chinese / Japanese in one pass.
- **QR codes** — `tomo>qr <text>` encodes, `tomo>qr` on an attached
  image decodes.
- **Statistics** — every command, message, and Gemini call is counted in
  the embedded LSM-tree DB (fjall, swappable via the `tomo_db::KvStore`
  trait).
- **Service framework** — the binary launches a `Vec<Box<dyn Service>>`.
  Today that's `DiscordService` + optional `RpcService` + optional
  `AdminService`. Adding another service is implementing one trait.

## Layout

```
tomo/
├── Cargo.toml             workspace root
├── .env.example           copy to `.env` and fill in
├── scripts/
│   ├── commands/*.rhai    user-invocable commands
│   └── triggers/*.rhai    auto-triggers
├── frontend/              Yew SPA (independent crate; built with trunk)
└── crates/
    ├── core/              shared types, config loader, Service trait
    ├── db/                async KvStore trait + fjall backend (bytes::Bytes)
    ├── cache/             persistent twilight-cache replacement
    ├── embed/             Embed2 builder + Embedable trait
    ├── pagination/        Paginator + PageSource (sync + lazy)
    ├── stats/             event counters via KvStore
    ├── gemini/            Gemini REST client with model fallback + cooldowns
    ├── requester/         outbound HTTP — booru, ehentai, kanji, nhentai,
    │                      urban, vndb, anilist
    ├── scripting/         Rhai engine, hot-reload, script + trigger registries
    ├── rpc/               tonic proto + generated client/server + auth
    ├── discord/           twilight bot service, dispatch, RPC server impl
    ├── admin/             axum web service: OAuth, REST API, serves Yew SPA
    └── tomo/              binary entry point — launches every enabled service
```

## Services

| Service          | Purpose                                                    |
| ---------------- | ---------------------------------------------------------- |
| `DiscordService` | Gateway, commands, triggers, gemini mention handler        |
| `RpcService`     | gRPC control plane backed by `BotState`                    |
| `AdminService`   | Web UI (Yew) + OAuth + REST API; gRPC client of the bot    |

`crates/tomo/src/main.rs` reads env toggles (`TOMO_ENABLE_RPC`,
`TOMO_ENABLE_ADMIN`) and only spins up what's enabled.

## Inter-service communication

Services talk to each other through **gRPC (tonic)** — see
`crates/rpc/proto/tomo.proto`. Today both servers live in the same process,
but the gRPC boundary means future services (Telegram bot, batch scrapers,
…) can run on a different host and reach the bot identically.

### Security

* **Discord bot:** owner-only commands gated by `BotState::is_owner`;
  master prefix only triggered for owners.
* **gRPC server:** binds to `127.0.0.1` by default; supports a Bearer
  token via `TOMO_RPC_TOKEN`. Privileged endpoints (`ReloadScripts`)
  re-check `is_owner` against the requester id.
* **Admin service:** binds to `127.0.0.1`; signed `HttpOnly`
  `SameSite=Lax` session cookies; OAuth with state + PKCE; ownership
  checked against the bot via gRPC (never trusts the client); CSP,
  X-Frame-Options=DENY, HSTS, Referrer-Policy on every response.
* **Yew frontend:** session cookie is `HttpOnly`; fetch uses
  `credentials: same-origin`; 401s redirect to `/login`.

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

The bot creates `./data/` (database) and watches `./scripts/` on first run.
The admin SPA is served from `./frontend/dist/`.

### Frontend development

```sh
cd frontend
trunk serve
```

This starts a dev server on `127.0.0.1:8081` with a proxy to the admin
backend at `127.0.0.1:8080`. Edits to Yew components hot-reload in the
browser.

### Required env vars

| Variable                | Default                    | Notes                                                  |
| ----------------------- | -------------------------- | ------------------------------------------------------ |
| `DISCORD_TOKEN`         | *required*                 | Bot token                                              |
| `TOMO_PREFIX`           | `tomo>`                    | Prefix for prefix-style commands (case-insensitive)    |
| `TOMO_MASTER_PREFIX`    | `%`                        | Owner-only prefix                                      |
| `TOMO_OWNERS`           | *app owner*                | Comma-separated user IDs                               |
| `TOMO_DATA_DIR`         | `./data`                   | fjall database directory                               |
| `TOMO_SCRIPT_DIR`       | `./scripts`                | Where to look for Rhai scripts                         |
| `GEMINI_API_KEY`        | *disables Gemini if unset* |                                                        |
| `GEMINI_MODEL`          | *flash chain*              | Comma-separated fallback chain (see below)             |
| `TOMO_OCR_*`            | *disables OCR if unset*    | Paths to PaddleOCR model files (latin / cjk pairs)     |

#### Gemini model fallback

`GEMINI_MODEL` accepts a comma-separated chain. The bot tries each model
in order; on a `429` (rate limit) it parses Google's `RetryInfo.retryDelay`
and parks that model for the cooldown, falling through to the next entry.
Defaults to the free-tier ladder (verified against
[Google's pricing page](https://ai.google.dev/gemini-api/docs/pricing)):

```
GEMINI_MODEL=gemini-2.5-flash,gemini-3-flash-preview,gemini-2.5-flash-lite,gemini-2.5-pro
```

Run `tomo>gemini` (owner-only) to see the chain state, including any
models currently in cooldown.

See `.env.example` for the full list of toggles
(`TOMO_ENABLE_PREFIX/SLASH/GEMINI/AUTO_TRIGGERS/HOT_RELOAD`,
`TOMO_REGISTER_GLOBAL`, `TOMO_GALLERY_LOOKUP_WAIT_SECS`, the OCR model
paths, RPC/admin/OAuth configuration).

## Built-in commands

| Category | Command                          | What it does                                                                |
| -------- | -------------------------------- | --------------------------------------------------------------------------- |
| General  | `ping`                           | Pong + latency                                                              |
| General  | `info`                           | Bot info, uptime, owners                                                    |
| General  | `invite`                         | OAuth invite URL                                                            |
| General  | `help`                           | Auto-generated help embed grouped by category                               |
| Utility  | `remind`                         | `remind <duration> <text>` / `remind list` / `remind remove <n>`            |
| Search   | `urban`  (`u`, `ud`)             | Urban Dictionary lookup                                                     |
| Search   | `kanji`  (`k`)                   | Kanji meanings + readings                                                   |
| Search   | `booru`                          | Random image from yandere / konachan / danbooru                             |
| Search   | `vndb`   (`vn`)                  | VNDB search by title or direct `vNNN` id / URL; lazy paginator              |
| Search   | `anime`  (`ani`, `al`) / `manga` | AniList GraphQL search; lazy paginator                                      |
| Search   | `nhentai` (`nh`, `nhen`)         | Gallery by id (NSFW channels only)                                          |
| Search   | `ehentai` (`eh`, `sadpanda`, …)  | Up to 25 galleries per call (NSFW only)                                     |
| Image    | `qr`     (`qrcode`)              | Encode text to a QR PNG, or decode an attached/replied image                |
| Image    | `ocr`                            | Extract text from an image (PaddleOCR)                                      |
| Stats    | `stats`                          | Global, per-user, per-server counters                                       |
| Stats    | `top`                            | Top 10 commands by use                                                      |
| Admin    | `reload`                         | Reload Rhai scripts on disk (owner only)                                    |
| Admin    | `gemini`                         | Check the Gemini chain + cooldowns (owner only)                             |

Auto-triggers (built-in, can be disabled with `TOMO_ENABLE_AUTO_TRIGGERS=false`):

| Trigger      | Reaction | When                                                                             |
| ------------ | -------- | -------------------------------------------------------------------------------- |
| `nhentai`    | 🕷       | NSFW channel, message body is *only* a number                                    |
| `ehentai`    | 🐼       | NSFW channel, message contains an E-Hentai `g/<gid>/<token>/` reference          |
| `vndb`       | 📖       | Any channel, message contains a `vNNN` id or `vndb.org/v…` URL                   |
| `gemini`     | (reply)  | Non-bot user `@`-mentions the bot, mentions a bot-role, or replies to its msg    |

Clicking the gallery / VN reaction within the wait window
(`TOMO_GALLERY_LOOKUP_WAIT_SECS`, default 30 s) makes the bot post the full
embed.

## Adding a Rhai command

```rhai
// scripts/commands/dice.rhai
fn meta() {
    #{
        name: "dice",
        description: "Roll a six-sided die.",
        aliases: ["d6"],
        category: "Utility",
    }
}

fn execute(ctx) {
    let r = random_int(1, 7);
    ctx.reply(`🎲 You rolled a **${r}**.`);
}
```

Save and the next message will see the new command — no restart.

`ctx` exposes: `channel_id`, `guild_id`, `user_id`, `message_id`, `args`,
`author_name`, `author_avatar_url`, `bot_name`, `bot_avatar_url`,
`now_unix`, `bot_started_at_unix`, `uptime_seconds`. Side-effects:
`reply`, `send`, `react`, `delete_invocation`, `log`,
`reply_embed`, `send_embed`.

Embed builder factories: `embed()`, `embed_info()`, `embed_success()`,
`embed_error()`, `embed_warning()`, `embed_lovely()`. All take chained
mutator calls (`e.title(...)`, `e.field_inline(...)`, etc.). Time helpers:
`format_time`, `format_time_in`, `format_duration`. RNG: `random_int`,
`random_float`. Safe parsers: `try_parse_int`, `try_parse_float` (return
`()` on failure — prefer these over Rhai's built-in `parse_int`, which
*raises* on bad input and aborts the script).

### Rhai gotchas

* `parse_int("foo")` **raises** in Rhai — use `try_parse_int` if the input
  is user-supplied.
* `trim()` is a **mutator** that returns `()`. Chain as a statement:
  ```rhai
  let raw = ctx.args;
  raw.trim();          // mutates in place
  ```
  Don't write `let raw = ctx.args.trim();` — `raw` will be `()`.
* `match` is a reserved keyword. In trigger metas, quote the key:
  ```rhai
  fn meta() { #{ name: "wave", "match": #{ regex: "hi" } } }
  ```
* Backticks inside backtick templates terminate the template. Don't write
  `` `text \`${var}\` more` `` — use straight quotes inside.

## Adding an auto-trigger

```rhai
// scripts/triggers/wave.rhai
fn meta() {
    #{
        name: "wave",
        "match": #{ regex: "(?i)\\b(hi|hello|hey)\\b" },
    }
}

fn execute(ctx) {
    ctx.react("👋");
}
```

Match shapes accepted: `regex`, `has_image`, `has_attachment`, `contains`,
`starts_with`, `mentions_bot`. See `scripts/README.md`.

## Owners

Tomo treats anyone listed in `TOMO_OWNERS` (or the application owner if
that variable is empty) as a bot owner. Owners can:

- Use the master prefix (`TOMO_MASTER_PREFIX`, default `%`) in addition
  to the normal prefix.
- Run `owner_only` commands like `reload` and `gemini`.

Flag a Rust command with `.owner_only()` in its `CommandMeta`, or check
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
                .category("General")
        });
        &META
    }
    async fn execute(&self, ctx: CommandContext) -> Result<()> {
        let up = ctx.bot.uptime();
        ctx.reply(&format!("Up for `{up}`")).await
    }
}
```

Register it in `crates/discord/src/command/builtin/mod.rs::all()`.

## Adding another service later

A new service (Telegram bot, web admin, etc.) just needs to implement
`tomo_core::Service` and get pushed onto the `services` vec in
`crates/tomo/src/main.rs`. Shared state (`db`, `config`, the persistent
cache via `tomo-cache`) is already cloneable.

## Swapping the database

The discord crate (and stats, cache, and any future crate) talks to
`Arc<dyn KvStore>`. To swap fjall for, say, SurrealDB or PostgreSQL:

1. Add `crates/db/src/your_backend.rs` implementing `KvStore`.
2. Pick it in `crates/tomo/src/main.rs`.

`tomo-cache` writes through that same trait, so the persistent twilight
cache moves backends with you.

## Tests

```sh
cargo test --workspace
```

Every layer has tests. Notable coverage:

* `tomo-embed` — empty-input filtering, so commands never accidentally
  ship Discord-rejecting payloads.
* `tomo-scripting` — script compilation, `meta()` parsing, and a
  `every_command_script_executes_without_error` smoke test that actually
  runs `execute(ctx)` on every shipped command.
* `tomo-requester` — parser corner cases for VNDB and AniList helpers.
* `tomo-gemini` — retry-delay parser (handles `47s`, `1.5s`, `1m30s`).
* `tomo-discord` — case-insensitive prefix matching, mention detection
  with content-scan fallback, VNDB id parsing in prose.

## License

MIT.
