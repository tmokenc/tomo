fn meta() {
    #{
        name: "uptime",
        description: "How long the bot has been up.",
        category: "General",
    }
}

fn execute(ctx) {
    let pretty = format_duration(ctx.uptime_seconds);
    let started = ctx.bot_started_at_unix;

    let e = embed_info();
    e.title(`⏱ Uptime`);
    e.description(`**${ctx.bot_name}** has been up for **${pretty}**.`);
    e.field_inline("Uptime", pretty);
    e.field_inline("Started", `<t:${started}:R>`);
    e.field_inline("Since", `<t:${started}:f>`);
    e.author_with(ctx.bot_name, ctx.bot_avatar_url, "");
    if ctx.bot_avatar_url != "" {
        e.thumbnail(ctx.bot_avatar_url);
    }
    e.footer_with(`${ctx.bot_name} · /uptime`, ctx.bot_avatar_url);
    e.timestamp_now();
    ctx.reply_embed(e);
}
