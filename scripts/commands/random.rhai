// Random number helper:
//   /random       -> float in [0, 1)
//   /random N     -> integer in [0, N)
//   /random A B   -> integer in [A, B)
// Anything else is echoed back.
fn meta() {
    #{
        name: "random",
        description: "Roll a random number.",
        aliases: ["rand", "rng"],
        category: "Utility",
    }
}

fn execute(ctx) {
    // Rhai's `trim()` mutates in-place and returns `()` — don't chain it.
    let raw = ctx.args;
    raw.trim();

    if raw == "" {
        ctx.reply(`🎲 ${random_float()}`);
        return;
    }

    let parts = raw.split(" ");
    if parts.len() == 1 {
        let n = try_parse_int(parts[0]);
        if n != () && n > 0 {
            ctx.reply(`🎲 ${random_int(0, n)} (from [0, ${n}))`);
            return;
        }
    }
    if parts.len() == 2 {
        let lo = try_parse_int(parts[0]);
        let hi = try_parse_int(parts[1]);
        if lo != () && hi != () && hi > lo {
            ctx.reply(`🎲 ${random_int(lo, hi)} (from [${lo}, ${hi}))`);
            return;
        }
    }

    // Note: backticks inside backtick templates terminate the template;
    // use plain quotes instead.
    ctx.reply(`Couldn't parse "${raw}" — try "random", "random N", or "random A B".`);
}
