fn meta() {
    #{
        name: "choose",
        description: "Pick one option at random from a `|`-separated list.",
        aliases: ["pick"],
        slash: false,
    }
}

fn execute(ctx) {
    let raw = ctx.args;
    if raw == "" {
        ctx.reply("Give me something to choose from — separate options with `|`.");
        return;
    }
    let options = raw.split("|");
    let trimmed = [];
    for opt in options {
        let t = opt.trim();
        if t != "" { trimmed.push(t); }
    }
    if trimmed.len() == 0 {
        ctx.reply("Nothing to choose from.");
        return;
    }
    let pick = trimmed[random_int(0, trimmed.len())];
    ctx.reply(`I choose **${pick}**.`);
}
