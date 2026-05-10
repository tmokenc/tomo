fn meta() {
    #{
        name: "coin",
        description: "Flip a coin.",
        aliases: ["flip"],
    }
}

fn execute(ctx) {
    let r = random_int(0, 2);
    let side = if r == 0 { "Heads" } else { "Tails" };
    ctx.embed("Coin flip", side, color_info());
}
