fn meta() {
    #{
        name: "picture",
        match: #{ has_image: true },
    }
}

fn execute(ctx) {
    ctx.react("📸");
}
