fn meta() {
    #{
        name: "roll",
        description: "Roll a die. Optional arg: number of sides (default 6).",
        aliases: ["dice"],
        category: "Utility",
    }
}

fn execute(ctx) {
    // Rhai's `trim()` mutates in-place and returns `()` — chain as a
    // statement, never assign its result.
    let raw = ctx.args;
    raw.trim();
    let sides = try_parse_int(raw);
    if sides == () || sides < 2 {
        sides = 6;
    }
    if sides > 1000000 {
        sides = 1000000;
    }
    let result = random_int(1, sides + 1);
    ctx.reply(`🎲 You rolled a **${result}** (d${sides}).`);
}
