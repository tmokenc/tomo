fn meta() {
    #{
        name: "roll",
        description: "Roll a die. Optional arg: number of sides (default 6).",
        aliases: ["dice"],
    }
}

fn execute(ctx) {
    let sides = parse_int(ctx.args);
    if sides == () || sides < 2 {
        sides = 6;
    }
    let result = random_int(1, sides + 1);
    ctx.reply(`🎲 You rolled a ${result} (d${sides}).`);
}
