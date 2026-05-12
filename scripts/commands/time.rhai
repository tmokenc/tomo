// Show the current time across a handful of common timezones. Passing a
// timestamp argument (Unix seconds) shows the time for that instant.
//
// When `MYON_USER_ID` is configured on the bot, a "Myon time" row is
// appended that tracks a specific user's awake/asleep state — see
// `crate::myon` in tomo-scripting.
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
        let label = entry[0];
        let tz = entry[1];
        // `format_offset_in` returns "+07" / "-05" — short and unambiguous.
        // Empty string means we couldn't resolve the tz, which would already
        // have shown up as a format error elsewhere.
        let offset = format_offset_in(tz, stamp);
        let value = format_time_in(tz, stamp, fmt);
        e.field_inline(`${label} (GMT${offset})`, value);
    }

    // Myon time — only shown when MYON_USER_ID is configured. The "GMT++"
    // label is preserved from the original tomoka-rs version: the hour
    // can go past 24 while the tracked user is still awake, so a real
    // GMT offset doesn't apply.
    if myon_enabled() {
        e.field_inline(`${myon_label()} (GMT++)`, myon_time(stamp, fmt));
    }

    e.field_block("Discord render", `<t:${stamp}:F> · <t:${stamp}:R>`);
    e.author_with(ctx.author_name, ctx.author_avatar_url, "");
    e.footer_with(`${ctx.bot_name} · /time`, ctx.bot_avatar_url);
    e.timestamp_unix(stamp);
    ctx.reply_embed(e);
}
