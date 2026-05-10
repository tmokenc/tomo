fn meta() {
    #{
        name: "nice",
        match: #{ regex: "\\b69\\b" },
    }
}

fn execute(ctx) {
    ctx.react("👍");
}
