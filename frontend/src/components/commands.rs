use yew::prelude::*;

use crate::api;
use crate::types::CommandInfo;

#[function_component(CommandsPage)]
pub fn commands_page() -> Html {
    let commands: UseStateHandle<Vec<CommandInfo>> = use_state(Vec::new);
    let error: UseStateHandle<Option<String>> = use_state(|| None);

    {
        let commands = commands.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::list_commands().await {
                    Ok(cs) => commands.set(cs),
                    Err(e) => error.set(Some(format!("{e}"))),
                }
            });
        });
    }

    html! {
        <>
            <h2>{"Commands"}</h2>
            if let Some(e) = error.as_ref() {
                <div class="error-banner">{ e }</div>
            }
            <table>
                <thead>
                    <tr>
                        <th>{"Name"}</th>
                        <th>{"Category"}</th>
                        <th>{"Description"}</th>
                        <th>{"Tags"}</th>
                    </tr>
                </thead>
                <tbody>
                    { for commands.iter().map(render_row) }
                </tbody>
            </table>
        </>
    }
}

fn render_row(c: &CommandInfo) -> Html {
    html! {
        <tr>
            <td>
                <code>{ &c.name }</code>
                if !c.aliases.is_empty() {
                    <span style="color: var(--muted); margin-left: 8px;">
                        { format!("({})", c.aliases.join(", ")) }
                    </span>
                }
            </td>
            <td>{ &c.category }</td>
            <td>{ &c.description }</td>
            <td>
                if c.slash      { <span class="badge slash">{"slash"}</span> }
                if c.prefix     { <span class="badge prefix">{"prefix"}</span> }
                if c.owner_only { <span class="badge owner">{"owner"}</span> }
                if c.is_script  { <span class="badge script">{"script"}</span> }
            </td>
        </tr>
    }
}
