// Show the current time across a handful of common timezones. Passing a
// timestamp argument (Unix seconds) shows the time for that instant.
fn meta() {
    #{
        name: "time",
        description: "Show the current time in several timezones.",
    }
}

fn execute(ctx) {
    let stamp = parse_int(ctx.args.trim());
    if stamp == () {
        stamp = ctx.now_unix;
    }

    let fmt = "%H:%M %d/%m/%Y";
    let zones = [
        ["UTC",          "UTC"],
        ["Pacific",      "US/Pacific"],
        ["Eastern",      "US/Eastern"],
        ["CET",          "CET"],
        ["Vietnam",      "Asia/Ho_Chi_Minh"],
        ["Japan",        "Asia/Tokyo"],
    ];

    let body = "";
    for entry in zones {
        let label = entry[0];
        let tz = entry[1];
        let value = format_time_in(tz, stamp, fmt);
        body += `**${label}** — \`${value}\`\n`;
    }
    body += `\nDiscord rendering: <t:${stamp}:F>`;

    ctx.embed(`Time`, body, color_info());
}
