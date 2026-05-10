fn meta() {
    #{
        name: "meow",
        match: #{ regex: "(?i)\\bmeow\\b" },
    }
}

fn execute(ctx) {
    ctx.react("🐱");
}
