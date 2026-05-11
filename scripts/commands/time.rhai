// Show the current time across a handful of common timezones. Passing a
// timestamp argument (Unix seconds) shows the time for that instant.
fn meta() {
    #{
        name: "time",
        description: "Show the current time in several timezones.",
        category: "Utility",
    }
}

fn execute(ctx) {
    // Rhai's `trim()` mutates in-place and returns `()` — don't chain it.
    let raw = ctx.args;
    raw.trim();
    let stamp = try_parse_int(raw);
    let explicit = stamp != ();
    if !explicit {
        stamp = ctx.now_unix;
    }

    let fmt = "%H:%M %d/%m/%Y";
    let zones = [
        ["🌐 UTC",      "UTC"],
        ["🇺🇸 Pacific", "US/Pacific"],
        ["🇺🇸 Eastern", "US/Eastern"],
        ["🇪🇺 CET",     "CET"],
        ["🇻🇳 Vietnam", "Asia/Ho_Chi_Minh"],
        ["🇯🇵 Japan",   "Asia/Tokyo"],
    ];

    let e = embed_info();
    e.title(`🕒 Time`);
    e.description(if explicit {
        `Showing the moment <t:${stamp}:F> (Unix ${stamp}).`
    } else {
        `Showing the current time (Unix ${stamp}).`
    });
    for entry in zones {
        e.field_inline(entry[0], format_time_in(entry[1], stamp, fmt));
    }
    e.field_block("Discord render", `<t:${stamp}:F> · <t:${stamp}:R>`);
    e.author_with(ctx.author_name, ctx.author_avatar_url, "");
    e.footer_with(`${ctx.bot_name} · /time`, ctx.bot_avatar_url);
    e.timestamp_unix(stamp);
    ctx.reply_embed(e);
}
