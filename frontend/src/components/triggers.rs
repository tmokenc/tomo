use yew::prelude::*;

use crate::api;
use crate::types::TriggerInfo;

#[function_component(TriggersPage)]
pub fn triggers_page() -> Html {
    let triggers: UseStateHandle<Vec<TriggerInfo>> = use_state(Vec::new);
    let error: UseStateHandle<Option<String>> = use_state(|| None);

    {
        let triggers = triggers.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::list_triggers().await {
                    Ok(t) => triggers.set(t),
                    Err(e) => error.set(Some(format!("{e}"))),
                }
            });
        });
    }

    html! {
        <>
            <h2>{"Auto-triggers"}</h2>
            if let Some(e) = error.as_ref() {
                <div class="error-banner">{ e }</div>
            }
            <table>
                <thead>
                    <tr>
                        <th>{"Name"}</th>
                        <th>{"Matcher"}</th>
                        <th>{"Argument"}</th>
                        <th>{"Source"}</th>
                    </tr>
                </thead>
                <tbody>
                    { for triggers.iter().map(|t| html! {
                        <tr>
                            <td><code>{ &t.name }</code></td>
                            <td>{ &t.matcher_kind }</td>
                            <td>{ if t.matcher_arg.is_empty() { "—".to_string() } else { format!("`{}`", t.matcher_arg) } }</td>
                            <td>
                                if t.is_script {
                                    <span class="badge script">{"script"}</span>
                                } else {
                                    <span class="badge">{"rust"}</span>
                                }
                            </td>
                        </tr>
                    }) }
                </tbody>
            </table>
        </>
    }
}
