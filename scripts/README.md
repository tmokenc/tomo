# Tomo scripts

Scripts in this directory are loaded by the Rhai engine at startup and again
whenever they change on disk (when `TOMO_ENABLE_HOT_RELOAD=true`).

## Layout

```
scripts/
├── commands/    one .rhai file per user-invocable command
└── triggers/    one .rhai file per auto-trigger
```

Only files with the exact `.rhai` extension load — anything else (e.g.
`*.rhai.example`) is ignored, so disabled examples can sit alongside live
scripts without firing. Rename `.rhai.example` → `.rhai` to enable one.

## Every script defines two functions

```rhai
// Returns a map describing this command/trigger. Required.
fn meta() {
    #{
        name: "ping",            // required
        description: "Pong!",    // optional
        aliases: ["p"],          // optional, commands only
        slash: true,             // optional, default true   (commands only)
        prefix: true,            // optional, default true   (commands only)
        guild_only: false,       // optional, default false  (commands only)
        // For triggers, replace command-specific fields with `match`.
        // See the trigger example below.
    }
}

// Called when the command is invoked / the trigger matches. Required.
fn execute(ctx) {
    ctx.reply("Pong!");
}
```

## What `ctx` exposes

Reads:
- `ctx.channel_id`, `ctx.guild_id`, `ctx.user_id`, `ctx.message_id` — i64
- `ctx.args` — string (whatever followed the command name, or the full
  message for triggers)
- `ctx.bot_name` — string

Side-effect methods (call these to do anything — direct IO is intentionally
not exposed):
- `ctx.reply(text)` — reply to the invocation
- `ctx.send(text)` — send a fresh message in the same channel
- `ctx.react(emoji)` — add a reaction
- `ctx.delete_invocation()` — delete the invoking message
- `ctx.reply_embed(e)` — reply with a built embed (see below)
- `ctx.send_embed(e)` — send (not reply) a fresh message with the embed
- `ctx.embed(title, description, color)` — shortcut for a 3-field embed reply
- `ctx.embed_footer(title, description, color, footer)` — shortcut + footer
- `ctx.log(text)` — write to the bot's tracing log

Constants:
- `color_info()`, `color_success()`, `color_error()`, `color_warning()`

Helpers:
- `random_int(min, max)` — exclusive-max-style pseudo-random number
- `random_float()` — float in [0, 1)
- `now_unix()` — current Unix time (seconds)
- `format_time(unix, fmt)` / `format_time_in(tz, unix, fmt)` — strftime in UTC / a named timezone
- `format_duration(seconds)` — humantime-style pretty duration

## Building an embed

`embed()` returns a fresh, mutable `Embed`. All methods auto-trim text to
Discord's per-field limits and silently drop the 26th field onwards.

```rhai
let e = embed();
e.title("Hello");
e.description("Body text");
e.color(color_info());
e.url("https://example.com");

// Author / footer
e.author("Tomo");
e.author_with("Tomo", "https://cdn/avatar.png", "https://example.com");
e.footer("powered by Tomo");
e.footer_with("powered by Tomo", "https://cdn/icon.png");

// Fields
e.field("Name", "Value", true);            // inline
e.field_inline("Score", "9000");           // shortcut
e.field_block("Notes", "wall of text");    // not inline

// Images
e.image("https://cdn/image.png");
e.thumbnail("https://cdn/thumb.png");
e.image_attachment("local_file.png");      // attachment://local_file.png
e.thumbnail_attachment("thumb.png");

// Timestamp
e.timestamp_now();
e.timestamp_unix(1700000000);

ctx.reply_embed(e);
```

## Trigger `match` shapes

`match` is a reserved keyword in Rhai, so it **must be quoted** as a map key:

```rhai
fn meta() {
    #{
        name: "wave",
        "match": #{ regex: "(?i)\\bhi\\b" },
    }
}
```

Accepted shapes:

```rhai
"match": #{ regex: "(?i)\\bhello\\b" }    // case-insensitive regex
"match": #{ has_image: true }             // any image attachment
"match": #{ has_attachment: true }        // any attachment
"match": #{ contains: "meow" }            // substring (case-insensitive)
"match": #{ starts_with: "!" }            // case-insensitive prefix
"match": #{ mentions_bot: true }          // user @-mentioned the bot
```

See `triggers/*.rhai.example` for working examples you can copy.
