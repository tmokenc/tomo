fn meta() {
    #{
        name: "choose",
        description: "Pick one option at random from a `|`-separated list.",
        aliases: ["pick"],
        slash: false,
        category: "Utility",
    }
}

fn execute(ctx) {
    let raw = ctx.args;
    if raw == "" {
        ctx.reply("Give me something to choose from — separate options with `|`. Example: `choose tea | coffee | water`.");
        return;
    }

    let options = raw.split("|");
    let trimmed = [];
    for opt in options {
        opt.trim();
        if opt != "" { trimmed.push(opt); }
    }
    if trimmed.len() == 0 {
        ctx.reply("Nothing to choose from.");
        return;
    }

    let pick = trimmed[random_int(0, trimmed.len())];
    ctx.reply(`🎯 I choose **${pick}**.`);
}
