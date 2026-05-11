fn meta() {
    // `match` is a reserved keyword in Rhai, so the key must be quoted.
    #{
        name: "meow",
        "match": #{ regex: "(?i)\\bmeow\\b" },
    }
}

fn execute(ctx) {
    ctx.react("🐱");
}
