use yew::prelude::*;

use crate::api;
use crate::types::{GlobalStats, Status, TopCommandEntry};

#[function_component(Dashboard)]
pub fn dashboard() -> Html {
    let status: UseStateHandle<Option<Status>> = use_state(|| None);
    let stats: UseStateHandle<GlobalStats> = use_state(GlobalStats::default);
    let top: UseStateHandle<Vec<TopCommandEntry>> = use_state(Vec::new);
    let error: UseStateHandle<Option<String>> = use_state(|| None);

    {
        let status = status.clone();
        let stats = stats.clone();
        let top = top.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::status().await {
                    Ok(s) => status.set(Some(s)),
                    Err(e) => error.set(Some(format!("status: {e}"))),
                }
                match api::global_stats().await {
                    Ok(g) => stats.set(g),
                    Err(e) => error.set(Some(format!("stats: {e}"))),
                }
                match api::top_commands().await {
                    Ok(t) => top.set(t.commands),
                    Err(e) => error.set(Some(format!("top: {e}"))),
                }
            });
        });
    }

    let reload = {
        let error = error.clone();
        Callback::from(move |_| {
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::reload_scripts().await {
                    Ok(r) => {
                        if r.success {
                            error.set(Some(format!(
                                "Reloaded ({} commands, {} triggers)",
                                r.command_count, r.trigger_count
                            )));
                        } else {
                            error.set(Some(format!("Reload failed: {}", r.message)));
                        }
                    }
                    Err(e) => error.set(Some(format!("Reload: {e}"))),
                }
            });
        })
    };

    html! {
        <>
            <h2>{"Dashboard"}</h2>
            if let Some(e) = error.as_ref() {
                <div class="error-banner">{ e }</div>
            }
            if let Some(s) = status.as_ref() {
                <div class="card">
                    <strong>{ &s.name }</strong>
                    {" — v"}{ &s.version }
                    {" · uptime "}
                    { format_uptime(s.uptime_seconds) }
                    {" · "}
                    if s.gemini_enabled { "Gemini ✓" } else { "Gemini —" }
                </div>
            }

            <div class="grid">
                <Stat label="Messages seen"  value={stats.messages} />
                <Stat label="Commands run"   value={stats.commands} />
                <Stat label="Gemini calls"   value={stats.gemini_calls} />
                <Stat label="Gemini tokens"  value={stats.gemini_tokens} />
            </div>

            <div class="card">
                <h3>{"Top commands"}</h3>
                if top.is_empty() {
                    <p class="loading">{"No commands recorded yet."}</p>
                } else {
                    <table>
                        <thead>
                            <tr><th>{"#"}</th><th>{"Command"}</th><th>{"Uses"}</th></tr>
                        </thead>
                        <tbody>
                            { for top.iter().enumerate().map(|(i, t)| html! {
                                <tr>
                                    <td>{ i + 1 }</td>
                                    <td><code>{ &t.command }</code></td>
                                    <td>{ t.count }</td>
                                </tr>
                            }) }
                        </tbody>
                    </table>
                }
            </div>

            <div class="card">
                <h3>{"Actions"}</h3>
                <button class="btn" onclick={reload}>{"Reload scripts"}</button>
            </div>
        </>
    }
}

#[derive(Properties, PartialEq)]
struct StatProps {
    label: &'static str,
    value: u64,
}

#[function_component(Stat)]
fn stat(p: &StatProps) -> Html {
    html! {
        <div class="stat">
            <div class="label">{ p.label }</div>
            <div class="value">{ p.value }</div>
        </div>
    }
}

fn format_uptime(secs: i64) -> String {
    let secs = secs.max(0) as u64;
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let mins = (secs % 3600) / 60;
    match (days, hours, mins) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, m) => format!("{d}d {h}h {m}m"),
    }
}
