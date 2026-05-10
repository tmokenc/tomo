// Tiny random helper. For numeric inputs:
//   /random          -> a float in [0, 1)
//   /random N        -> integer in [0, N)
//   /random A B      -> integer in [A, B)
// Otherwise the input is echoed back unchanged. (The richer `Nd`/`Nh`
// placeholder syntax from tomoka-rs lives in the Rust pattern helper if you
// need it back — see the README.)
fn meta() {
    #{
        name: "random",
        description: "Roll a random number.",
        aliases: ["rand", "rng"],
    }
}

fn execute(ctx) {
    let raw = ctx.args.trim();
    if raw == "" {
        ctx.reply(`${random_float()}`);
        return;
    }
    let parts = raw.split(" ");
    if parts.len() == 1 {
        let n = parse_int(parts[0]);
        if n != () && n > 0 {
            ctx.reply(`${random_int(0, n)}`);
            return;
        }
    }
    if parts.len() == 2 {
        let lo = parse_int(parts[0]);
        let hi = parse_int(parts[1]);
        if lo != () && hi != () && hi > lo {
            ctx.reply(`${random_int(lo, hi)}`);
            return;
        }
    }
    ctx.reply(raw);
}
