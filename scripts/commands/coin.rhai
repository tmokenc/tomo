fn meta() {
    #{
        name: "coin",
        description: "Flip a coin.",
        aliases: ["flip"],
        category: "Utility",
    }
}

fn execute(ctx) {
    let heads = random_int(0, 2) == 0;
    let face = if heads { "🪙 Heads" } else { "🌑 Tails" };
    ctx.reply(face);
}
