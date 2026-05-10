fn meta() {
    #{
        name: "echo",
        description: "Repeat whatever you say back at you.",
        slash: false,   // slash needs declared options — scripts only use prefix for now.
    }
}

fn execute(ctx) {
    let text = ctx.args;
    if text == "" {
        ctx.reply("…you didn't give me anything to echo.");
    } else {
        ctx.reply(text);
    }
}
