fn meta() {
    #{
        name: "uptime",
        description: "How long the bot has been up.",
    }
}

fn execute(ctx) {
    let pretty = format_duration(ctx.uptime_seconds);

    let e = embed();
    e.title("Uptime");
    e.description(`I have been up for **${pretty}**.`);
    e.color(color_info());
    e.timestamp_now();
    ctx.reply_embed(e);
}
